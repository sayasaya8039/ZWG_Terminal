const std = @import("std");

const SimdVec = @Vector(32, u8);
const MaskVec = @Vector(32, u1);
const full_chunk_mask: u32 = 0xFFFF_FFFF;
pub const ContentKind = enum(u8) {
    unknown = 0,
    plain_text = 1,
    json = 2,
    markdown = 3,
};

pub const flag_json_root: u8 = 1 << 0;
pub const flag_markdown_code_fence: u8 = 1 << 1;
pub const flag_markdown_line_marker: u8 = 1 << 2;

pub const ContentStats = extern struct {
    kind: u8,
    flags: u8,
    reserved0: u8,
    reserved1: u8,
    json_structural_count: u32,
    markdown_marker_count: u32,
};

fn isWhitespace(byte: u8) bool {
    return switch (byte) {
        ' ', '\t', '\n', '\r' => true,
        else => false,
    };
}

inline fn maskForByte(chunk: SimdVec, needle: u8) u32 {
    const matches = @select(
        u1,
        chunk == @as(SimdVec, @splat(needle)),
        @as(MaskVec, @splat(@as(u1, 1))),
        @as(MaskVec, @splat(@as(u1, 0))),
    );
    return @bitCast(matches);
}

inline fn nonWhitespaceMask(chunk: SimdVec) u32 {
    const whitespace_mask = maskForByte(chunk, ' ') |
        maskForByte(chunk, '\t') |
        maskForByte(chunk, '\n') |
        maskForByte(chunk, '\r');
    return (~whitespace_mask) & full_chunk_mask;
}

inline fn jsonStructuralMask(chunk: SimdVec) u32 {
    return maskForByte(chunk, '{') |
        maskForByte(chunk, '}') |
        maskForByte(chunk, '[') |
        maskForByte(chunk, ']') |
        maskForByte(chunk, ':') |
        maskForByte(chunk, ',');
}

inline fn markdownMask(chunk: SimdVec) u32 {
    return maskForByte(chunk, '`') |
        maskForByte(chunk, '#') |
        maskForByte(chunk, '-') |
        maskForByte(chunk, '*') |
        maskForByte(chunk, '>') |
        maskForByte(chunk, '|');
}

inline fn rangeMask(start: usize, end: usize) u32 {
    if (start >= end) return 0;

    const wide_full = @as(u64, full_chunk_mask);
    const start_mask = if (start == 0)
        @as(u64, 0)
    else
        (wide_full >> @as(u6, @intCast(32 - start)));
    const end_mask = if (end >= 32)
        wide_full
    else
        (wide_full >> @as(u6, @intCast(32 - end)));
    return @as(u32, @intCast(end_mask & ~start_mask));
}

inline fn escapedQuoteMask(
    chunk_bytes: [32]u8,
    quote_mask: u32,
    odd_backslash_run: bool,
) u32 {
    var escaped_mask: u32 = 0;
    var remaining = quote_mask;

    while (remaining != 0) {
        const lane: usize = @intCast(@ctz(remaining));
        var backslash_count: u32 = 0;
        var cursor = lane;
        while (cursor > 0 and chunk_bytes[cursor - 1] == '\\') : (cursor -= 1) {
            backslash_count += 1;
        }
        if (cursor == 0 and odd_backslash_run) {
            backslash_count += 1;
        }
        if ((backslash_count & 1) == 1) {
            escaped_mask |= @as(u32, 1) << @as(u5, @intCast(lane));
        }

        remaining &= remaining - 1;
    }

    return escaped_mask;
}

const StringMaskResult = struct {
    mask: u32,
    end_in_string: bool,
};

inline fn buildStringMask(unescaped_quote_mask: u32, start_in_string: bool) StringMaskResult {
    var mask: u32 = 0;
    var remaining = unescaped_quote_mask;
    var current_in_string = start_in_string;
    var cursor: usize = 0;

    while (remaining != 0) {
        const lane: usize = @intCast(@ctz(remaining));
        if (current_in_string) {
            mask |= rangeMask(cursor, lane);
        }
        current_in_string = !current_in_string;
        cursor = lane + 1;
        remaining &= remaining - 1;
    }

    if (current_in_string) {
        mask |= rangeMask(cursor, 32);
    }

    return .{
        .mask = mask,
        .end_in_string = current_in_string,
    };
}

inline fn trailingOddBackslashRun(
    chunk_bytes: [32]u8,
    start_in_string: bool,
    end_in_string: bool,
    start_odd_backslash_run: bool,
    unescaped_quote_mask: u32,
) bool {
    if (!end_in_string) return false;

    var index: usize = 32;
    var backslash_count: u32 = 0;
    while (index > 0 and chunk_bytes[index - 1] == '\\') : (index -= 1) {
        backslash_count += 1;
    }

    if (index == 0 and start_in_string and unescaped_quote_mask == 0) {
        return ((backslash_count & 1) == 1) != start_odd_backslash_run;
    }

    return (backslash_count & 1) == 1;
}

inline fn resetBacktickRunOnGap(backtick_run: *u8, previous_index: ?usize, current_index: usize) void {
    if (backtick_run.* == 0) return;
    if (previous_index == null) {
        if (current_index > 0) backtick_run.* = 0;
        return;
    }
    if (current_index != previous_index.? + 1) {
        backtick_run.* = 0;
    }
}

pub fn detectContent(input: []const u8) ContentStats {
    var first_significant: ?u8 = null;
    var in_string = false;
    var odd_backslash_run = false;
    var line_start = true;
    var backtick_run: u8 = 0;
    var json_structural_count: u32 = 0;
    var markdown_marker_count: u32 = 0;
    var flags: u8 = 0;
    var i: usize = 0;

    while (i + 32 <= input.len) {
        const start_in_string = in_string;
        const start_odd_backslash_run = odd_backslash_run;
        const chunk: SimdVec = @as(*align(1) const [32]u8, @ptrCast(input.ptr + i)).*;
        const chunk_bytes: [32]u8 = @bitCast(chunk);
        const quote_mask = maskForByte(chunk, '"');
        const unescaped_quote_mask = quote_mask & ~escapedQuoteMask(chunk_bytes, quote_mask, start_odd_backslash_run);
        const string_mask_info = buildStringMask(unescaped_quote_mask, in_string);
        const newline_mask = maskForByte(chunk, '\n') | maskForByte(chunk, '\r');
        const structural_mask = jsonStructuralMask(chunk) & ~string_mask_info.mask;
        const markdown_mask = markdownMask(chunk) & ~string_mask_info.mask;
        const visible_newline_mask = newline_mask & ~string_mask_info.mask;
        const visible_non_whitespace_mask = nonWhitespaceMask(chunk) & ~string_mask_info.mask;

        var scan_mask = unescaped_quote_mask | visible_newline_mask | structural_mask | markdown_mask;
        if (first_significant == null or line_start) {
            scan_mask |= visible_non_whitespace_mask;
        }

        if (scan_mask == 0) {
            in_string = string_mask_info.end_in_string;
            odd_backslash_run = trailingOddBackslashRun(
                chunk_bytes,
                start_in_string,
                string_mask_info.end_in_string,
                start_odd_backslash_run,
                unescaped_quote_mask,
            );
            i += 32;
            continue;
        }

        var previous_index: ?usize = null;
        while (scan_mask != 0) {
            const lane: usize = @intCast(@ctz(scan_mask));
            const byte = chunk_bytes[lane];
            resetBacktickRunOnGap(&backtick_run, previous_index, lane);

            if (first_significant == null and !isWhitespace(byte)) {
                first_significant = byte;
                if (byte == '{' or byte == '[') flags |= flag_json_root;
            }

            switch (byte) {
                '"' => {
                    in_string = true;
                    line_start = false;
                    backtick_run = 0;
                },
                '{', '}', '[', ']', ':', ',' => {
                    json_structural_count += 1;
                    line_start = false;
                    backtick_run = 0;
                },
                '\n' => {
                    line_start = true;
                    backtick_run = 0;
                },
                '\r' => {
                    backtick_run = 0;
                },
                '`' => {
                    backtick_run +|= 1;
                    if (backtick_run == 3) {
                        markdown_marker_count += 3;
                        flags |= flag_markdown_code_fence;
                    }
                    line_start = false;
                },
                '#', '-', '*', '>' => {
                    backtick_run = 0;
                    if (line_start) {
                        markdown_marker_count += 2;
                        flags |= flag_markdown_line_marker;
                    }
                    line_start = false;
                },
                '|' => {
                    markdown_marker_count += 1;
                    line_start = false;
                    backtick_run = 0;
                },
                ' ', '\t' => {
                    backtick_run = 0;
                },
                else => {
                    line_start = false;
                    backtick_run = 0;
                },
            }

            previous_index = lane;
            scan_mask &= scan_mask - 1;
        }

        in_string = string_mask_info.end_in_string;
        odd_backslash_run = trailingOddBackslashRun(
            chunk_bytes,
            start_in_string,
            string_mask_info.end_in_string,
            start_odd_backslash_run,
            unescaped_quote_mask,
        );

        if (!in_string and backtick_run > 0 and previous_index.? < 31) {
            backtick_run = 0;
        }

        i += 32;
    }

    while (i < input.len) : (i += 1) {
        const byte = input[i];

        if (first_significant == null and !isWhitespace(byte)) {
            first_significant = byte;
            if (byte == '{' or byte == '[') flags |= flag_json_root;
        }

        if (in_string) {
            if (odd_backslash_run) {
                odd_backslash_run = false;
                continue;
            }
            if (byte == '\\') {
                odd_backslash_run = true;
                continue;
            }
            if (byte == '"') {
                in_string = false;
            }
            continue;
        }

        switch (byte) {
            '"' => {
                in_string = true;
                line_start = false;
                backtick_run = 0;
            },
            '{', '}', '[', ']', ':', ',' => {
                json_structural_count += 1;
                line_start = false;
                backtick_run = 0;
            },
            '\n' => {
                line_start = true;
                backtick_run = 0;
            },
            '\r' => {
                backtick_run = 0;
            },
            '`' => {
                backtick_run +|= 1;
                if (backtick_run == 3) {
                    markdown_marker_count += 3;
                    flags |= flag_markdown_code_fence;
                }
                line_start = false;
            },
            '#', '-', '*', '>' => {
                backtick_run = 0;
                if (line_start) {
                    markdown_marker_count += 2;
                    flags |= flag_markdown_line_marker;
                }
                line_start = false;
            },
            '|' => {
                markdown_marker_count += 1;
                line_start = false;
                backtick_run = 0;
            },
            ' ', '\t' => {
                backtick_run = 0;
            },
            else => {
                line_start = false;
                backtick_run = 0;
            },
        }
    }

    const kind: ContentKind = if ((flags & flag_json_root) != 0 and json_structural_count >= 3 and markdown_marker_count * 2 <= json_structural_count + 1)
        .json
    else if (markdown_marker_count >= 3 and ((flags & flag_markdown_code_fence) != 0 or (flags & flag_markdown_line_marker) != 0 or markdown_marker_count > json_structural_count))
        .markdown
    else if (first_significant != null)
        .plain_text
    else
        .unknown;

    return .{
        .kind = @intFromEnum(kind),
        .flags = flags,
        .reserved0 = 0,
        .reserved1 = 0,
        .json_structural_count = json_structural_count,
        .markdown_marker_count = markdown_marker_count,
    };
}
