const std = @import("std");

const SimdVec = @Vector(32, u8);
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

fn isInterestingOutsideString(chunk: SimdVec) bool {
    inline for ([_]u8{ '"', '\\', '\n', '\r', '{', '}', '[', ']', ':', ',', '`', '#', '-', '*', '>', '|' }) |needle| {
        if (@reduce(.Or, chunk == @as(SimdVec, @splat(needle)))) {
            return true;
        }
    }
    return false;
}

fn isInterestingInsideString(chunk: SimdVec) bool {
    return @reduce(.Or, chunk == @as(SimdVec, @splat(@as(u8, '"')))) or
        @reduce(.Or, chunk == @as(SimdVec, @splat(@as(u8, '\\'))));
}

pub fn detectContent(input: []const u8) ContentStats {
    var first_significant: ?u8 = null;
    var in_string = false;
    var escape = false;
    var line_start = true;
    var backtick_run: u8 = 0;
    var json_structural_count: u32 = 0;
    var markdown_marker_count: u32 = 0;
    var flags: u8 = 0;
    var i: usize = 0;

    while (i + 32 <= input.len) {
        const chunk: SimdVec = @as(*align(1) const [32]u8, @ptrCast(input.ptr + i)).*;

        if (in_string) {
            if (!isInterestingInsideString(chunk)) {
                i += 32;
                continue;
            }
        } else if (!line_start and first_significant != null and !isInterestingOutsideString(chunk)) {
            i += 32;
            continue;
        }

        const end = i + 32;
        while (i < end) : (i += 1) {
            const byte = input[i];

            if (first_significant == null and !isWhitespace(byte)) {
                first_significant = byte;
                if (byte == '{' or byte == '[') flags |= flag_json_root;
            }

            if (in_string) {
                if (escape) {
                    escape = false;
                    continue;
                }
                if (byte == '\\') {
                    escape = true;
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
    }

    while (i < input.len) : (i += 1) {
        const byte = input[i];

        if (first_significant == null and !isWhitespace(byte)) {
            first_significant = byte;
            if (byte == '{' or byte == '[') flags |= flag_json_root;
        }

        if (in_string) {
            if (escape) {
                escape = false;
                continue;
            }
            if (byte == '\\') {
                escape = true;
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
