const std = @import("std");

const Artifact = enum { ghostty, lib };

pub fn build(b: *std.Build) void {
    const optimize = b.standardOptimizeOption(.{});
    const target = b.standardTargetOptions(.{});

    // --- uucode dependency ---
    const uucode_config_path = b.path("uucode_config.zig");

    // Get uucode tables path
    const uucode_dep = b.dependency("uucode", .{
        .build_config_path = uucode_config_path,
    });
    const uucode_tables = uucode_dep.namedLazyPath("tables.zig");

    // uucode module for target
    const uucode_target = b.dependency("uucode", .{
        .target = target,
        .optimize = optimize,
        .tables_path = uucode_tables,
        .build_config_path = uucode_config_path,
    });

    // --- Unicode table generators (host) ---
    const uucode_host = b.dependency("uucode", .{
        .target = b.graph.host,
        .tables_path = uucode_tables,
        .build_config_path = uucode_config_path,
    });

    const props_exe = b.addExecutable(.{
        .name = "props-unigen",
        .root_module = b.createModule(.{
            .root_source_file = b.path("ghostty_src/unicode/props_uucode.zig"),
            .target = b.graph.host,
        }),
        .use_llvm = true,
    });
    props_exe.root_module.addImport("uucode", uucode_host.module("uucode"));

    const symbols_exe = b.addExecutable(.{
        .name = "symbols-unigen",
        .root_module = b.createModule(.{
            .root_source_file = b.path("ghostty_src/unicode/symbols_uucode.zig"),
            .target = b.graph.host,
        }),
        .use_llvm = true,
    });
    symbols_exe.root_module.addImport("uucode", uucode_host.module("uucode"));

    const props_run = b.addRunArtifact(props_exe);
    const symbols_run = b.addRunArtifact(symbols_exe);

    const props_output = props_run.captureStdOut();
    const symbols_output = symbols_run.captureStdOut();

    // --- Main static library ---
    const lib = b.addLibrary(.{
        .name = "ghostty_vt",
        .root_module = b.createModule(.{
            .root_source_file = b.path("lib.zig"),
            .target = target,
            .optimize = optimize,
        }),
        .linkage = .static,
    });
    lib.linkLibC();

    // DX12 GPU renderer system libraries (Windows only)
    lib.linkSystemLibrary("d3d12");
    lib.linkSystemLibrary("dxgi");
    lib.linkSystemLibrary("d3dcompiler");
    lib.linkSystemLibrary("dwrite");
    lib.linkSystemLibrary("gdi32");

    // terminal_options
    const terminal_opts = b.addOptions();
    terminal_opts.addOption(Artifact, "artifact", .lib);
    terminal_opts.addOption(bool, "c_abi", false);
    terminal_opts.addOption(bool, "oniguruma", false);
    terminal_opts.addOption(bool, "simd", true);
    terminal_opts.addOption(bool, "slow_runtime_safety", false);
    terminal_opts.addOption(bool, "kitty_graphics", false);
    terminal_opts.addOption(bool, "tmux_control_mode", false);
    lib.root_module.addOptions("terminal_options", terminal_opts);

    // build_options
    const build_opts = b.addOptions();
    build_opts.addOption(bool, "simd", true);
    lib.root_module.addOptions("build_options", build_opts);

    // Unicode tables - copy generated stdout to named .zig files
    const wf = b.addWriteFiles();
    const props_zig = wf.addCopyFile(props_output, "unicode_tables.zig");
    const symbols_zig = wf.addCopyFile(symbols_output, "symbols_tables.zig");
    lib.step.dependOn(&wf.step);
    lib.root_module.addAnonymousImport("unicode_tables", .{
        .root_source_file = props_zig,
    });
    lib.root_module.addAnonymousImport("symbols_tables", .{
        .root_source_file = symbols_zig,
    });

    // uucode module
    lib.root_module.addImport("uucode", uucode_target.module("uucode"));

    // --- Install ---
    const lib_install = b.addInstallLibFile(lib.getEmittedBin(), "libghostty_vt.a");
    b.getInstallStep().dependOn(&lib_install.step);
}
