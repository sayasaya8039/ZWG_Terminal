"""
ZWG Terminal Installer Builder
Stages release files and runs PyInstaller to create a single-EXE installer.
"""

import os
import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
STAGE = ROOT / "installer" / "_stage"
RELEASE_EXE = ROOT / "target" / "release" / "zwg.exe"
RESOURCES = ROOT / "resources"
ICON = RESOURCES / "icons" / "zwg.ico"
INSTALLER_SCRIPT = ROOT / "installer" / "zwg_installer.py"


def stage_files():
    """Copy release artifacts into _stage/ for embedding."""
    if STAGE.exists():
        shutil.rmtree(STAGE)
    STAGE.mkdir(parents=True)

    # Main binary
    if not RELEASE_EXE.exists():
        print(f"ERROR: {RELEASE_EXE} not found. Run 'cargo build --release' first.")
        sys.exit(1)
    shutil.copy2(RELEASE_EXE, STAGE / "zwg.exe")

    # Resources
    stage_res = STAGE / "resources"
    shutil.copytree(RESOURCES, stage_res)

    # Uninstaller script (embedded)
    shutil.copy2(ROOT / "installer" / "zwg_uninstaller.py", STAGE / "zwg_uninstaller.py")

    print(f"Staged files in {STAGE}")
    for p in sorted(STAGE.rglob("*")):
        if p.is_file():
            rel = p.relative_to(STAGE)
            sz = p.stat().st_size
            print(f"  {rel}  ({sz:,} bytes)")


def run_pyinstaller():
    """Build the installer EXE with PyInstaller."""
    dist_dir = ROOT / "installer" / "dist"
    build_dir = ROOT / "installer" / "build"

    cmd = [
        sys.executable, "-m", "PyInstaller",
        "--onefile",
        "--windowed",
        "--name", "ZWG_Terminal_Setup",
        f"--icon={ICON}",
        f"--add-data={STAGE};_stage",
        "--distpath", str(dist_dir),
        "--workpath", str(build_dir),
        "--specpath", str(ROOT / "installer"),
        "--clean",
        str(INSTALLER_SCRIPT),
    ]

    print("\nRunning PyInstaller...")
    print(" ".join(cmd))
    subprocess.check_call(cmd)

    output = dist_dir / "ZWG_Terminal_Setup.exe"
    if output.exists():
        sz_mb = output.stat().st_size / (1024 * 1024)
        print(f"\nInstaller created: {output}")
        print(f"Size: {sz_mb:.1f} MB")
    else:
        print("ERROR: Installer EXE not found after build.")
        sys.exit(1)


def main():
    os.chdir(ROOT)
    stage_files()
    run_pyinstaller()


if __name__ == "__main__":
    main()
