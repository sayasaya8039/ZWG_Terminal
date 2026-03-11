const std = @import("std");
const terminal = @import("ghostty_src/terminal/main.zig");
const ghostty_input = @import("ghostty_src/input.zig");

const Allocator = std.mem.Allocator;

const TerminalHandle = struct {
    alloc: Allocator,
    terminal: terminal.Terminal,
    stream: terminal.Stream(*Handler),
    handler: Handler,
    default_fg: terminal.color.RGB,
    default_bg: terminal.color.RGB,
    viewport_top_y_screen: u32,
    has_viewport_top_y_screen: bool,

    fn init(alloc: Allocator, cols: u16, rows: u16) !*TerminalHandle {
        return initWithOptions(alloc, cols, rows, null);
    }

    fn initWithScrollback(
        alloc: Allocator,
        cols: u16,
        rows: u16,
        max_scrollback: usize,
    ) !*TerminalHandle {
        return initWithOptions(alloc, cols, rows, max_scrollback);
    }

    fn initWithOptions(
        alloc: Allocator,
        cols: u16,
        rows: u16,
        max_scrollback: ?usize,
    ) !*TerminalHandle {
        const handle = try alloc.create(TerminalHandle);
        errdefer alloc.destroy(handle);

        const t = if (max_scrollback) |limit|
            try terminal.Terminal.init(alloc, .{
                .cols = cols,
                .rows = rows,
                .max_scrollback = limit,
            })
        else
            try terminal.Terminal.init(alloc, .{
                .cols = cols,
                .rows = rows,
            });
        errdefer {
            var tmp = t;
            tmp.deinit(alloc);
        }

        handle.* = .{
            .alloc = alloc,
            .terminal = t,
            .handler = .{ .terminal = undefined },
            .stream = undefined,
            .default_fg = .{ .r = 0xFF, .g = 0xFF, .b = 0xFF },
            .default_bg = .{ .r = 0x00, .g = 0x00, .b = 0x00 },
            .viewport_top_y_screen = 0,
            .has_viewport_top_y_screen = true,
        };
        handle.handler.terminal = &handle.terminal;
        handle.stream = terminal.Stream(*Handler).initAlloc(alloc, &handle.handler);
        return handle;
    }

    fn deinit(self: *TerminalHandle) void {
        self.stream.deinit();
        self.terminal.deinit(self.alloc);
        self.alloc.destroy(self);
    }
};

const Handler = struct {
    terminal: *terminal.Terminal,

    pub fn deinit(self: *Handler) void {
        _ = self;
    }

    /// Generic VT action handler - dispatches all terminal actions
    pub fn vt(
        self: *Handler,
        comptime action: anytype,
        value: anytype,
    ) !void {
        switch (action) {
            .print => try self.terminal.print(value.cp),
            .backspace => self.terminal.backspace(),
            .horizontal_tab => {
                const count: usize = @intCast(value);
                for (0..count) |_| {
                    self.terminal.horizontalTab();
                }
            },
            .linefeed => try self.terminal.linefeed(),
            .carriage_return => self.terminal.carriageReturn(),
            .set_attribute => try self.terminal.setAttribute(value),
            .configure_charset => self.terminal.configureCharset(value.slot, value.charset),
            .invoke_charset => self.terminal.invokeCharset(value.bank, value.charset, value.locking),
            .cursor_left => self.terminal.cursorLeft(value.value),
            .cursor_right => self.terminal.cursorRight(value.value),
            .cursor_down => {
                self.terminal.cursorDown(value.value);
            },
            .cursor_up => {
                self.terminal.cursorUp(value.value);
            },
            .cursor_col => self.terminal.setCursorPos(
                self.terminal.screens.active.cursor.y + 1,
                value.value,
            ),
            .cursor_row => self.terminal.setCursorPos(
                value.value,
                self.terminal.screens.active.cursor.x + 1,
            ),
            .cursor_pos => self.terminal.setCursorPos(value.row, value.col),
            .erase_display_below,
            .erase_display_above,
            .erase_display_complete,
            .erase_display_scrollback,
            .erase_display_scroll_complete,
            => {
                const mode: terminal.EraseDisplay = switch (action) {
                    .erase_display_below => .below,
                    .erase_display_above => .above,
                    .erase_display_complete => .complete,
                    .erase_display_scrollback => .scrollback,
                    .erase_display_scroll_complete => .scroll_complete,
                    else => unreachable,
                };
                self.terminal.eraseDisplay(mode, value);
            },
            .erase_line_right,
            .erase_line_left,
            .erase_line_complete,
            => {
                const mode: terminal.EraseLine = switch (action) {
                    .erase_line_right => .right,
                    .erase_line_left => .left,
                    .erase_line_complete => .complete,
                    else => unreachable,
                };
                self.terminal.eraseLine(mode, value);
            },
            .start_hyperlink => try self.terminal.screens.active.startHyperlink(value.uri, value.id),
            .end_hyperlink => self.terminal.screens.active.endHyperlink(),
            .set_mode => {
                self.terminal.modes.set(value.mode, true);
            },
            .reset_mode => {
                self.terminal.modes.set(value.mode, false);
            },
            .color_operation => {
                if (value.requests.count() == 0) return;
                var it = value.requests.constIterator(0);
                while (it.next()) |req| {
                    switch (req.*) {
                        .set => |set| switch (set.target) {
                            .palette => |i| {
                                self.terminal.colors.palette.set(i, set.color);
                                self.terminal.flags.dirty.palette = true;
                            },
                            else => {},
                        },
                        .reset => |target| switch (target) {
                            .palette => |i| {
                                self.terminal.colors.palette.reset(i);
                                self.terminal.flags.dirty.palette = true;
                            },
                            else => {},
                        },
                        .reset_palette => {
                            self.terminal.colors.palette.resetAll();
                            self.terminal.flags.dirty.palette = true;
                        },
                        else => {},
                    }
                }
            },
            else => {},
        }
    }
};

// --- C ABI exports ---

const ghostty_vt_bytes_t = extern struct {
    ptr: ?[*]const u8,
    len: usize,
};

const CellStyle = extern struct {
    fg_r: u8,
    fg_g: u8,
    fg_b: u8,
    bg_r: u8,
    bg_g: u8,
    bg_b: u8,
    flags: u8,
    reserved: u8,
};

const StyleRun = extern struct {
    start_col: u16,
    end_col: u16,
    fg_r: u8,
    fg_g: u8,
    fg_b: u8,
    bg_r: u8,
    bg_g: u8,
    bg_b: u8,
    flags: u8,
    reserved: u8,
};

export fn ghostty_vt_terminal_new(cols: u16, rows: u16) callconv(.c) ?*anyopaque {
    const alloc = std.heap.c_allocator;
    const handle = TerminalHandle.init(alloc, cols, rows) catch return null;
    return @ptrCast(handle);
}

export fn ghostty_vt_terminal_new_with_scrollback(
    cols: u16,
    rows: u16,
    max_scrollback: usize,
) callconv(.c) ?*anyopaque {
    const alloc = std.heap.c_allocator;
    const handle = TerminalHandle.initWithScrollback(alloc, cols, rows, max_scrollback) catch return null;
    return @ptrCast(handle);
}

export fn ghostty_vt_terminal_free(terminal_ptr: ?*anyopaque) callconv(.c) void {
    if (terminal_ptr == null) return;
    const handle: *TerminalHandle = @ptrCast(@alignCast(terminal_ptr.?));
    handle.deinit();
}

export fn ghostty_vt_terminal_set_default_colors(
    terminal_ptr: ?*anyopaque,
    fg_r: u8,
    fg_g: u8,
    fg_b: u8,
    bg_r: u8,
    bg_g: u8,
    bg_b: u8,
) callconv(.c) void {
    if (terminal_ptr == null) return;
    const handle: *TerminalHandle = @ptrCast(@alignCast(terminal_ptr.?));
    handle.default_fg = .{ .r = fg_r, .g = fg_g, .b = fg_b };
    handle.default_bg = .{ .r = bg_r, .g = bg_g, .b = bg_b };
}

export fn ghostty_vt_terminal_feed(
    terminal_ptr: ?*anyopaque,
    bytes: [*]const u8,
    len: usize,
) callconv(.c) c_int {
    if (terminal_ptr == null) return 1;
    const handle: *TerminalHandle = @ptrCast(@alignCast(terminal_ptr.?));

    handle.stream.nextSlice(bytes[0..len]) catch return 2;
    return 0;
}

export fn ghostty_vt_terminal_resize(
    terminal_ptr: ?*anyopaque,
    cols: u16,
    rows: u16,
) callconv(.c) c_int {
    if (terminal_ptr == null) return 1;
    const handle: *TerminalHandle = @ptrCast(@alignCast(terminal_ptr.?));

    handle.terminal.resize(
        handle.alloc,
        @as(terminal.size.CellCountInt, @intCast(cols)),
        @as(terminal.size.CellCountInt, @intCast(rows)),
    ) catch return 2;
    return 0;
}

export fn ghostty_vt_terminal_scroll_viewport(
    terminal_ptr: ?*anyopaque,
    delta_lines: i32,
) callconv(.c) c_int {
    if (terminal_ptr == null) return 1;
    const handle: *TerminalHandle = @ptrCast(@alignCast(terminal_ptr.?));
    handle.terminal.scrollViewport(.{ .delta = @as(isize, delta_lines) });
    return 0;
}

export fn ghostty_vt_terminal_scroll_viewport_top(terminal_ptr: ?*anyopaque) callconv(.c) c_int {
    if (terminal_ptr == null) return 1;
    const handle: *TerminalHandle = @ptrCast(@alignCast(terminal_ptr.?));
    handle.terminal.scrollViewport(.top);
    return 0;
}

export fn ghostty_vt_terminal_scroll_viewport_bottom(terminal_ptr: ?*anyopaque) callconv(.c) c_int {
    if (terminal_ptr == null) return 1;
    const handle: *TerminalHandle = @ptrCast(@alignCast(terminal_ptr.?));
    handle.terminal.scrollViewport(.bottom);
    return 0;
}

export fn ghostty_vt_terminal_cursor_position(
    terminal_ptr: ?*anyopaque,
    col_out: ?*u16,
    row_out: ?*u16,
) callconv(.c) bool {
    if (terminal_ptr == null) return false;
    if (col_out == null or row_out == null) return false;
    const handle: *TerminalHandle = @ptrCast(@alignCast(terminal_ptr.?));
    const screen = handle.terminal.screens.active;
    col_out.?.* = @intCast(screen.cursor.x + 1);
    row_out.?.* = @intCast(screen.cursor.y + 1);
    return true;
}

export fn ghostty_vt_terminal_dump_viewport(terminal_ptr: ?*anyopaque) callconv(.c) ghostty_vt_bytes_t {
    if (terminal_ptr == null) return .{ .ptr = null, .len = 0 };
    const handle: *TerminalHandle = @ptrCast(@alignCast(terminal_ptr.?));

    const alloc = std.heap.c_allocator;
    const screen = handle.terminal.screens.active;
    const slice = screen.dumpStringAlloc(alloc, .{ .viewport = .{} }) catch {
        return .{ .ptr = null, .len = 0 };
    };
    return .{ .ptr = slice.ptr, .len = slice.len };
}

export fn ghostty_vt_terminal_dump_viewport_row(
    terminal_ptr: ?*anyopaque,
    row: u16,
) callconv(.c) ghostty_vt_bytes_t {
    if (terminal_ptr == null) return .{ .ptr = null, .len = 0 };
    const handle: *TerminalHandle = @ptrCast(@alignCast(terminal_ptr.?));

    const screen = handle.terminal.screens.active;
    const cols = handle.terminal.cols;
    const tl_pt: terminal.point.Point = .{ .viewport = .{ .x = 0, .y = row } };
    const tl_pin = screen.pages.pin(tl_pt) orelse return .{ .ptr = null, .len = 0 };
    const br_pt: terminal.point.Point = .{ .viewport = .{ .x = if (cols > 0) cols - 1 else 0, .y = row } };
    const br_pin = screen.pages.pin(br_pt) orelse return .{ .ptr = null, .len = 0 };

    const alloc = std.heap.c_allocator;
    var builder: std.Io.Writer.Allocating = .init(alloc);
    defer builder.deinit();

    screen.dumpString(&builder.writer, .{
        .tl = tl_pin,
        .br = br_pin,
        .unwrap = false,
    }) catch return .{ .ptr = null, .len = 0 };

    const slice = builder.toOwnedSlice() catch return .{ .ptr = null, .len = 0 };
    return .{ .ptr = slice.ptr, .len = slice.len };
}

fn resolveStyle(
    s: terminal.Style,
    cell: *const terminal.page.Cell,
    default_fg: terminal.color.RGB,
    default_bg: terminal.color.RGB,
    palette: *const terminal.color.Palette,
) CellStyle {
    var fg = s.fg(.{ .default = default_fg, .palette = palette, .bold = null });
    var bg = s.bg(cell, palette) orelse default_bg;

    var flags_byte: u8 = 0;
    if (s.flags.inverse) flags_byte |= 0x01;
    if (s.flags.bold) flags_byte |= 0x02;
    if (s.flags.italic) flags_byte |= 0x04;
    if (s.flags.underline != .none) flags_byte |= 0x08;
    if (s.flags.faint) flags_byte |= 0x10;
    if (s.flags.invisible) flags_byte |= 0x20;
    if (s.flags.strikethrough) flags_byte |= 0x40;

    if (s.flags.inverse) {
        const tmp = fg;
        fg = bg;
        bg = tmp;
    }
    if (s.flags.invisible) {
        fg = bg;
    }

    return .{
        .fg_r = fg.r, .fg_g = fg.g, .fg_b = fg.b,
        .bg_r = bg.r, .bg_g = bg.g, .bg_b = bg.b,
        .flags = flags_byte,
        .reserved = 0,
    };
}

export fn ghostty_vt_terminal_dump_viewport_row_cell_styles(
    terminal_ptr: ?*anyopaque,
    row: u16,
) callconv(.c) ghostty_vt_bytes_t {
    if (terminal_ptr == null) return .{ .ptr = null, .len = 0 };
    const handle: *TerminalHandle = @ptrCast(@alignCast(terminal_ptr.?));

    const screen = handle.terminal.screens.active;
    const pt: terminal.point.Point = .{ .viewport = .{ .x = 0, .y = row } };
    const pin = screen.pages.pin(pt) orelse return .{ .ptr = null, .len = 0 };
    const cells = pin.cells(.all);

    const default_fg = handle.default_fg;
    const default_bg = handle.default_bg;
    const palette: *const terminal.color.Palette = &handle.terminal.colors.palette.current;

    const alloc = std.heap.c_allocator;
    var out: std.ArrayList(u8) = .empty;
    errdefer out.deinit(alloc);
    out.ensureTotalCapacity(alloc, cells.len * @sizeOf(CellStyle)) catch return .{ .ptr = null, .len = 0 };

    for (cells) |*cell| {
        const s = pin.style(cell);
        const rec = resolveStyle(s, cell, default_fg, default_bg, palette);
        out.appendSlice(alloc, std.mem.asBytes(&rec)) catch return .{ .ptr = null, .len = 0 };
    }

    const slice = out.toOwnedSlice(alloc) catch return .{ .ptr = null, .len = 0 };
    return .{ .ptr = slice.ptr, .len = slice.len };
}

export fn ghostty_vt_terminal_dump_viewport_row_style_runs(
    terminal_ptr: ?*anyopaque,
    row: u16,
) callconv(.c) ghostty_vt_bytes_t {
    if (terminal_ptr == null) return .{ .ptr = null, .len = 0 };
    const handle: *TerminalHandle = @ptrCast(@alignCast(terminal_ptr.?));

    const screen = handle.terminal.screens.active;
    const pt: terminal.point.Point = .{ .viewport = .{ .x = 0, .y = row } };
    const pin = screen.pages.pin(pt) orelse return .{ .ptr = null, .len = 0 };
    const cells = pin.cells(.all);

    const default_fg = handle.default_fg;
    const default_bg = handle.default_bg;
    const palette: *const terminal.color.Palette = &handle.terminal.colors.palette.current;

    const alloc = std.heap.c_allocator;
    var out: std.ArrayList(u8) = .empty;
    errdefer out.deinit(alloc);

    if (cells.len == 0) {
        const slice = out.toOwnedSlice(alloc) catch return .{ .ptr = null, .len = 0 };
        return .{ .ptr = slice.ptr, .len = slice.len };
    }

    var prev = resolveStyle(pin.style(&cells[0]), &cells[0], default_fg, default_bg, palette);
    var run_start: u16 = 1;

    var col_idx: usize = 1;
    while (col_idx < cells.len) : (col_idx += 1) {
        const cell = &cells[col_idx];
        const cur = resolveStyle(pin.style(cell), cell, default_fg, default_bg, palette);

        const same = cur.fg_r == prev.fg_r and cur.fg_g == prev.fg_g and cur.fg_b == prev.fg_b and
            cur.bg_r == prev.bg_r and cur.bg_g == prev.bg_g and cur.bg_b == prev.bg_b and
            cur.flags == prev.flags;

        if (!same) {
            const rec = StyleRun{
                .start_col = run_start,
                .end_col = @intCast(col_idx),
                .fg_r = prev.fg_r, .fg_g = prev.fg_g, .fg_b = prev.fg_b,
                .bg_r = prev.bg_r, .bg_g = prev.bg_g, .bg_b = prev.bg_b,
                .flags = prev.flags,
                .reserved = 0,
            };
            out.appendSlice(alloc, std.mem.asBytes(&rec)) catch return .{ .ptr = null, .len = 0 };
            run_start = @intCast(col_idx + 1);
            prev = cur;
        }
    }

    // Emit final run
    const last = StyleRun{
        .start_col = run_start,
        .end_col = @intCast(cells.len),
        .fg_r = prev.fg_r, .fg_g = prev.fg_g, .fg_b = prev.fg_b,
        .bg_r = prev.bg_r, .bg_g = prev.bg_g, .bg_b = prev.bg_b,
        .flags = prev.flags,
        .reserved = 0,
    };
    out.appendSlice(alloc, std.mem.asBytes(&last)) catch return .{ .ptr = null, .len = 0 };

    const slice = out.toOwnedSlice(alloc) catch return .{ .ptr = null, .len = 0 };
    return .{ .ptr = slice.ptr, .len = slice.len };
}

export fn ghostty_vt_terminal_take_dirty_viewport_rows(
    terminal_ptr: ?*anyopaque,
    rows: u16,
) callconv(.c) ghostty_vt_bytes_t {
    if (terminal_ptr == null or rows == 0) return .{ .ptr = null, .len = 0 };
    const handle: *TerminalHandle = @ptrCast(@alignCast(terminal_ptr.?));

    const alloc = std.heap.c_allocator;
    var out: std.ArrayList(u8) = .empty;
    errdefer out.deinit(alloc);

    const dirty = handle.terminal.flags.dirty;
    const force_full_redraw = dirty.clear or dirty.palette or dirty.reverse_colors or dirty.preedit;
    if (force_full_redraw) {
        handle.terminal.flags.dirty.clear = false;
        handle.terminal.flags.dirty.palette = false;
        handle.terminal.flags.dirty.reverse_colors = false;
        handle.terminal.flags.dirty.preedit = false;
    }

    const screen = handle.terminal.screens.active;
    var y: u32 = 0;
    while (y < rows) : (y += 1) {
        const pt: terminal.point.Point = .{ .viewport = .{ .x = 0, .y = y } };
        const pin = screen.pages.pin(pt) orelse continue;
        if (!force_full_redraw and !pin.isDirty()) continue;

        const v: u16 = @intCast(y);
        out.append(alloc, @intCast(v & 0xFF)) catch return .{ .ptr = null, .len = 0 };
        out.append(alloc, @intCast((v >> 8) & 0xFF)) catch return .{ .ptr = null, .len = 0 };

        // Clear dirty flag for this row
        pin.rowAndCell().row.dirty = false;
    }

    const slice = out.toOwnedSlice(alloc) catch return .{ .ptr = null, .len = 0 };
    return .{ .ptr = slice.ptr, .len = slice.len };
}

fn pinScreenRow(pin: terminal.PageList.Pin) u32 {
    var y: u32 = @intCast(pin.y);
    var node_ = pin.node;
    while (node_.prev) |node| {
        y += @intCast(node.data.size.rows);
        node_ = node;
    }
    return y;
}

export fn ghostty_vt_terminal_take_viewport_scroll_delta(
    terminal_ptr: ?*anyopaque,
) callconv(.c) i32 {
    if (terminal_ptr == null) return 0;
    const handle: *TerminalHandle = @ptrCast(@alignCast(terminal_ptr.?));

    const screen = handle.terminal.screens.active;
    const tl = screen.pages.getTopLeft(.viewport);
    const current: u32 = pinScreenRow(tl);

    if (!handle.has_viewport_top_y_screen) {
        handle.viewport_top_y_screen = current;
        handle.has_viewport_top_y_screen = true;
        return 0;
    }

    const prev: u32 = handle.viewport_top_y_screen;
    handle.viewport_top_y_screen = current;

    const delta64: i64 = @as(i64, @intCast(current)) - @as(i64, @intCast(prev));
    if (delta64 > std.math.maxInt(i32)) return std.math.maxInt(i32);
    if (delta64 < std.math.minInt(i32)) return std.math.minInt(i32);
    return @intCast(delta64);
}

export fn ghostty_vt_terminal_hyperlink_at(
    terminal_ptr: ?*anyopaque,
    col: u16,
    row: u16,
) callconv(.c) ghostty_vt_bytes_t {
    if (terminal_ptr == null or col == 0 or row == 0) return .{ .ptr = null, .len = 0 };
    const handle: *TerminalHandle = @ptrCast(@alignCast(terminal_ptr.?));

    const screen = handle.terminal.screens.active;
    const x: terminal.size.CellCountInt = @intCast(col - 1);
    const y: u32 = @intCast(row - 1);
    const pt: terminal.point.Point = .{ .viewport = .{ .x = x, .y = y } };
    const pin = screen.pages.pin(pt) orelse return .{ .ptr = null, .len = 0 };
    const rac = pin.rowAndCell();
    if (!rac.cell.hyperlink) return .{ .ptr = null, .len = 0 };

    const id = pin.node.data.lookupHyperlink(rac.cell) orelse return .{ .ptr = null, .len = 0 };
    const entry = pin.node.data.hyperlink_set.get(pin.node.data.memory, id).*;
    const uri = entry.uri.offset.ptr(pin.node.data.memory)[0..entry.uri.len];

    const alloc = std.heap.c_allocator;
    const duped = alloc.dupe(u8, uri) catch return .{ .ptr = null, .len = 0 };
    return .{ .ptr = duped.ptr, .len = duped.len };
}

export fn ghostty_vt_encode_key_named(
    name_ptr: ?[*]const u8,
    name_len: usize,
    modifiers: u16,
) callconv(.c) ghostty_vt_bytes_t {
    if (name_ptr == null or name_len == 0) return .{ .ptr = null, .len = 0 };

    const name = name_ptr.?[0..name_len];

    const key_value: ghostty_input.Key = if (std.mem.eql(u8, name, "up"))
        .arrow_up
    else if (std.mem.eql(u8, name, "down"))
        .arrow_down
    else if (std.mem.eql(u8, name, "left"))
        .arrow_left
    else if (std.mem.eql(u8, name, "right"))
        .arrow_right
    else if (std.mem.eql(u8, name, "home"))
        .home
    else if (std.mem.eql(u8, name, "end"))
        .end
    else if (std.mem.eql(u8, name, "pageup") or std.mem.eql(u8, name, "page_up") or std.mem.eql(u8, name, "page-up"))
        .page_up
    else if (std.mem.eql(u8, name, "pagedown") or std.mem.eql(u8, name, "page_down") or std.mem.eql(u8, name, "page-down"))
        .page_down
    else if (std.mem.eql(u8, name, "insert"))
        .insert
    else if (std.mem.eql(u8, name, "delete"))
        .delete
    else if (std.mem.eql(u8, name, "backspace"))
        .backspace
    else if (std.mem.eql(u8, name, "enter"))
        .enter
    else if (std.mem.eql(u8, name, "tab"))
        .tab
    else if (std.mem.eql(u8, name, "escape"))
        .escape
    else if (name.len >= 2 and name[0] == 'f')
        parse_function_key(name[1..]) orelse return .{ .ptr = null, .len = 0 }
    else
        return .{ .ptr = null, .len = 0 };

    var mods: ghostty_input.Mods = .{};
    if ((modifiers & 0x0001) != 0) mods.shift = true;
    if ((modifiers & 0x0002) != 0) mods.ctrl = true;
    if ((modifiers & 0x0004) != 0) mods.alt = true;
    if ((modifiers & 0x0008) != 0) mods.super = true;

    const event: ghostty_input.KeyEvent = .{
        .action = .press,
        .key = key_value,
        .mods = mods,
    };

    const opts: ghostty_input.key_encode.Options = .{
        .alt_esc_prefix = true,
    };

    // Use a fixed buffer writer
    var buf: [128]u8 = undefined;
    var writer: std.Io.Writer = .fixed(&buf);
    ghostty_input.key_encode.encode(&writer, event, opts) catch return .{ .ptr = null, .len = 0 };
    const written_len = writer.end;
    if (written_len == 0) return .{ .ptr = null, .len = 0 };

    const alloc = std.heap.c_allocator;
    const duped = alloc.dupe(u8, buf[0..written_len]) catch return .{ .ptr = null, .len = 0 };
    return .{ .ptr = duped.ptr, .len = duped.len };
}

fn parse_function_key(digits: []const u8) ?ghostty_input.Key {
    if (digits.len == 1) {
        return switch (digits[0]) {
            '1' => .f1,
            '2' => .f2,
            '3' => .f3,
            '4' => .f4,
            '5' => .f5,
            '6' => .f6,
            '7' => .f7,
            '8' => .f8,
            '9' => .f9,
            else => null,
        };
    }

    if (digits.len == 2 and digits[0] == '1') {
        return switch (digits[1]) {
            '0' => .f10,
            '1' => .f11,
            '2' => .f12,
            else => null,
        };
    }

    return null;
}

export fn ghostty_vt_bytes_free(bytes: ghostty_vt_bytes_t) callconv(.c) void {
    if (bytes.ptr == null or bytes.len == 0) return;
    std.heap.c_allocator.free(bytes.ptr.?[0..bytes.len]);
}

// Ghostty's terminal stream uses this symbol as an optimization hook.
export fn ghostty_simd_decode_utf8_until_control_seq(
    input: [*]const u8,
    count: usize,
    output: [*]u32,
    output_count: *usize,
) callconv(.c) usize {
    var i: usize = 0;
    var out_i: usize = 0;
    while (i < count) {
        if (input[i] == 0x1B) break;

        const b0 = input[i];
        var cp: u32 = 0xFFFD;
        var need: usize = 1;

        if (b0 < 0x80) {
            cp = b0;
            need = 1;
        } else if (b0 & 0xE0 == 0xC0) {
            need = 2;
            if (i + need > count) break;
            const b1 = input[i + 1];
            if (b1 & 0xC0 != 0x80) {
                cp = 0xFFFD;
                need = 1;
            } else {
                cp = ((@as(u32, b0 & 0x1F)) << 6) | (@as(u32, b1 & 0x3F));
            }
        } else if (b0 & 0xF0 == 0xE0) {
            need = 3;
            if (i + need > count) break;
            const b1 = input[i + 1];
            const b2 = input[i + 2];
            if (b1 & 0xC0 != 0x80 or b2 & 0xC0 != 0x80) {
                cp = 0xFFFD;
                need = 1;
            } else {
                cp = ((@as(u32, b0 & 0x0F)) << 12) |
                    ((@as(u32, b1 & 0x3F)) << 6) |
                    (@as(u32, b2 & 0x3F));
            }
        } else if (b0 & 0xF8 == 0xF0) {
            need = 4;
            if (i + need > count) break;
            const b1 = input[i + 1];
            const b2 = input[i + 2];
            const b3 = input[i + 3];
            if (b1 & 0xC0 != 0x80 or b2 & 0xC0 != 0x80 or b3 & 0xC0 != 0x80) {
                cp = 0xFFFD;
                need = 1;
            } else {
                cp = ((@as(u32, b0 & 0x07)) << 18) |
                    ((@as(u32, b1 & 0x3F)) << 12) |
                    ((@as(u32, b2 & 0x3F)) << 6) |
                    (@as(u32, b3 & 0x3F));
            }
        } else {
            cp = 0xFFFD;
            need = 1;
        }

        output[out_i] = cp;
        out_i += 1;
        i += need;
    }

    output_count.* = out_i;
    return i;
}
