//! Lock-free SPSC ring buffer + async parser thread for PTY I/O
//!
//! Architecture:
//!   PTY Reader (Rust) → SPSC ring buffer push (lock-free, non-blocking)
//!                          ↓
//!   Parser Thread (Zig) → drain buffer → lock terminal mutex → feed → unlock
//!
//! This eliminates the bottleneck of PTY reader blocking on terminal parsing
//! under a mutex, enabling zero-contention I/O for AI CLI JSON/Markdown streams.

const std = @import("std");

/// 4MB ring buffer capacity (power of 2 for bitmask modulo)
pub const RING_CAPACITY: usize = 4 * 1024 * 1024;
const RING_MASK: usize = RING_CAPACITY - 1;
/// Coalesce window: start conservatively at 2ms, but the parser loop
/// adaptively shortens to 1ms when sustained output is detected (e.g.
/// Claude Code /fast mode).  Reverts to 2ms after an idle period.
const OUTPUT_COALESCE_NS_NORMAL: u64 = 2 * std.time.ns_per_ms;
const OUTPUT_COALESCE_NS_FAST: u64 = 1 * std.time.ns_per_ms;
/// Consecutive busy cycles before switching to fast coalesce mode.
const FAST_MODE_THRESHOLD: u32 = 8;
/// Idle cycles before reverting to normal coalesce mode.
const FAST_MODE_REVERT_IDLE: u32 = 200;
const DRAIN_BUFFER_CAPACITY: usize = 512 * 1024;

/// Single-Producer Single-Consumer lock-free ring buffer.
///
/// Producer: PTY reader thread (Rust side, via feed_async C API)
/// Consumer: Parser thread (Zig side, drains to terminal)
///
/// Lock-free: uses only atomic load/store on write_pos and read_pos.
/// No CAS or LL/SC needed for SPSC.
pub const SpscRingBuffer = struct {
    buf: [*]u8,
    write_pos: usize = 0,
    read_pos: usize = 0,

    pub fn init(alloc: std.mem.Allocator) !*SpscRingBuffer {
        const self = try alloc.create(SpscRingBuffer);
        const buf = try alloc.alloc(u8, RING_CAPACITY);
        self.* = .{
            .buf = buf.ptr,
        };
        return self;
    }

    pub fn deinit(self: *SpscRingBuffer, alloc: std.mem.Allocator) void {
        alloc.free(self.buf[0..RING_CAPACITY]);
        alloc.destroy(self);
    }

    /// Push data into the ring buffer (producer side).
    /// Returns number of bytes actually written.
    /// Non-blocking — silently drops excess if buffer is full.
    pub fn push(self: *SpscRingBuffer, data: []const u8) usize {
        const wp = @atomicLoad(usize, &self.write_pos, .monotonic);
        const rp = @atomicLoad(usize, &self.read_pos, .acquire);
        const used = wp -% rp;
        const free = RING_CAPACITY - used;
        const n = @min(data.len, free);
        if (n == 0) return 0;

        const start = wp & RING_MASK;
        const first_chunk = @min(n, RING_CAPACITY - start);
        @memcpy(self.buf[start .. start + first_chunk], data[0..first_chunk]);
        if (n > first_chunk) {
            const rest = n - first_chunk;
            @memcpy(self.buf[0..rest], data[first_chunk .. first_chunk + rest]);
        }

        @atomicStore(usize, &self.write_pos, wp +% n, .release);
        return n;
    }

    /// Pop data from the ring buffer (consumer side).
    /// Returns number of bytes read into `out`.
    pub fn pop(self: *SpscRingBuffer, out: []u8) usize {
        const rp = @atomicLoad(usize, &self.read_pos, .monotonic);
        const wp = @atomicLoad(usize, &self.write_pos, .acquire);
        const pending = wp -% rp;
        const n = @min(pending, out.len);
        if (n == 0) return 0;

        const start = rp & RING_MASK;
        const first_chunk = @min(n, RING_CAPACITY - start);
        @memcpy(out[0..first_chunk], self.buf[start .. start + first_chunk]);
        if (n > first_chunk) {
            const rest = n - first_chunk;
            @memcpy(out[first_chunk .. first_chunk + rest], self.buf[0..rest]);
        }

        @atomicStore(usize, &self.read_pos, rp +% n, .release);
        return n;
    }

    /// Number of bytes pending in the buffer.
    pub fn pendingBytes(self: *const SpscRingBuffer) usize {
        const wp = @atomicLoad(usize, &self.write_pos, .acquire);
        const rp = @atomicLoad(usize, &self.read_pos, .acquire);
        return wp -% rp;
    }
};

/// Function signature for feeding data to the terminal.
/// Decoupled from TerminalHandle to avoid circular imports.
pub const FeedFn = *const fn (ctx: *anyopaque, data: [*]const u8, len: usize) void;

/// Async feeder: owns SPSC ring buffer + parser thread.
///
/// The parser thread drains the ring buffer, locks the terminal mutex,
/// and feeds data to the terminal via the provided FeedFn callback.
pub const AsyncFeeder = struct {
    ring: *SpscRingBuffer,
    thread: ?std.Thread = null,
    stop_flag: bool = false,
    new_data_flag: bool = false,
    feed_fn: FeedFn,
    feed_ctx: *anyopaque,
    terminal_mutex: *std.Thread.Mutex,

    pub fn init(
        alloc: std.mem.Allocator,
        feed_fn: FeedFn,
        feed_ctx: *anyopaque,
        mutex: *std.Thread.Mutex,
    ) !*AsyncFeeder {
        const ring = try SpscRingBuffer.init(alloc);
        errdefer ring.deinit(alloc);
        const self = try alloc.create(AsyncFeeder);
        self.* = .{
            .ring = ring,
            .feed_fn = feed_fn,
            .feed_ctx = feed_ctx,
            .terminal_mutex = mutex,
        };
        return self;
    }

    /// Start the parser thread.
    pub fn start(self: *AsyncFeeder) !void {
        if (self.thread != null) return; // already running
        self.thread = try std.Thread.spawn(.{}, parserLoop, .{self});
    }

    /// Signal stop and join the parser thread.
    pub fn stop(self: *AsyncFeeder) void {
        @atomicStore(bool, &self.stop_flag, true, .release);
        if (self.thread) |t| {
            t.join();
            self.thread = null;
        }
    }

    pub fn deinit(self: *AsyncFeeder, alloc: std.mem.Allocator) void {
        self.stop();
        self.ring.deinit(alloc);
        alloc.destroy(self);
    }

    /// Check and clear the new-data flag (called from UI thread).
    pub fn takeNewData(self: *AsyncFeeder) bool {
        const had = @atomicLoad(bool, &self.new_data_flag, .acquire);
        if (had) {
            @atomicStore(bool, &self.new_data_flag, false, .release);
        }
        return had;
    }

    fn parserLoop(self: *AsyncFeeder) void {
        // 512KB drain buffer to coalesce PTY bursts into a single feed.
        var drain_buf: [DRAIN_BUFFER_CAPACITY]u8 = undefined;
        var idle_count: u32 = 0;
        var busy_count: u32 = 0;

        while (!@atomicLoad(bool, &self.stop_flag, .acquire)) {
            const n = self.ring.pop(&drain_buf);
            if (n > 0) {
                idle_count = 0;
                busy_count +|= 1;

                // Adaptive coalesce: shorten window under sustained output
                const coalesce_ns = if (busy_count >= FAST_MODE_THRESHOLD)
                    OUTPUT_COALESCE_NS_FAST
                else
                    OUTPUT_COALESCE_NS_NORMAL;

                var total = n;
                std.Thread.sleep(coalesce_ns);
                while (total < drain_buf.len) {
                    const drained = self.ring.pop(drain_buf[total..]);
                    if (drained == 0) break;
                    total += drained;
                }
                self.terminal_mutex.lock();
                self.feed_fn(self.feed_ctx, drain_buf[0..total].ptr, total);
                self.terminal_mutex.unlock();
                @atomicStore(bool, &self.new_data_flag, true, .release);
            } else {
                // Adaptive sleep — minimize latency for bursts, save CPU when idle
                idle_count +|= 1;
                if (idle_count >= FAST_MODE_REVERT_IDLE) {
                    busy_count = 0;
                }
                if (idle_count < 100) {
                    std.atomic.spinLoopHint();
                } else if (idle_count < 1000) {
                    std.Thread.yield() catch {};
                } else {
                    std.Thread.sleep(50_000); // 50μs
                }
            }
        }

        // Drain remaining data before thread exits
        while (true) {
            const n = self.ring.pop(&drain_buf);
            if (n == 0) break;
            self.terminal_mutex.lock();
            self.feed_fn(self.feed_ctx, drain_buf[0..n].ptr, n);
            self.terminal_mutex.unlock();
        }
    }
};
