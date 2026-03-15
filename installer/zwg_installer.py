"""
ZWG Terminal Installer — GUI installer for Windows.
Packaged by PyInstaller into a single EXE with all assets embedded.
"""

import ctypes
import os
import shutil
import subprocess
import sys
import threading
import winreg
from pathlib import Path
from tkinter import (
    BOTH,
    BOTTOM,
    CENTER,
    DISABLED,
    END,
    FLAT,
    LEFT,
    NORMAL,
    RIGHT,
    TOP,
    W,
    X,
    Y,
    BooleanVar,
    Button,
    Checkbutton,
    Entry,
    Frame,
    Label,
    StringVar,
    Text,
    Tk,
    filedialog,
    messagebox,
)
from tkinter.ttk import Progressbar

APP_NAME = "ZWG Terminal"
APP_VERSION = "1.1.1"
APP_EXE = "zwg.exe"
PUBLISHER = "ZWG Terminal contributors"
UNINSTALL_KEY = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\ZWGTerminal"

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def is_admin() -> bool:
    try:
        return ctypes.windll.shell32.IsUserAnAdmin() != 0
    except Exception:
        return False


def get_stage_dir() -> Path:
    """Return the _stage directory embedded by PyInstaller."""
    if getattr(sys, "frozen", False):
        return Path(sys._MEIPASS) / "_stage"
    return Path(__file__).resolve().parent / "_stage"


def create_shortcut(shortcut_path: str, target: str, icon: str, working_dir: str):
    """Create a Windows .lnk shortcut via PowerShell."""
    ps_script = (
        f'$ws = New-Object -ComObject WScript.Shell; '
        f'$sc = $ws.CreateShortcut("{shortcut_path}"); '
        f'$sc.TargetPath = "{target}"; '
        f'$sc.WorkingDirectory = "{working_dir}"; '
        f'$sc.IconLocation = "{icon}"; '
        f'$sc.Save()'
    )
    subprocess.run(
        ["powershell.exe", "-NoProfile", "-Command", ps_script],
        capture_output=True,
    )


def register_uninstall(install_dir: Path, uninstaller: Path):
    """Register in Windows Add/Remove Programs."""
    try:
        key = winreg.CreateKeyEx(winreg.HKEY_CURRENT_USER, UNINSTALL_KEY)
        winreg.SetValueEx(key, "DisplayName", 0, winreg.REG_SZ, APP_NAME)
        winreg.SetValueEx(key, "DisplayVersion", 0, winreg.REG_SZ, APP_VERSION)
        winreg.SetValueEx(key, "Publisher", 0, winreg.REG_SZ, PUBLISHER)
        winreg.SetValueEx(key, "InstallLocation", 0, winreg.REG_SZ, str(install_dir))
        winreg.SetValueEx(key, "DisplayIcon", 0, winreg.REG_SZ, str(install_dir / "resources" / "icons" / "zwg.ico"))
        winreg.SetValueEx(key, "UninstallString", 0, winreg.REG_SZ, str(uninstaller))
        winreg.SetValueEx(key, "NoModify", 0, winreg.REG_DWORD, 1)
        winreg.SetValueEx(key, "NoRepair", 0, winreg.REG_DWORD, 1)
        winreg.CloseKey(key)
    except OSError:
        pass


# ---------------------------------------------------------------------------
# Installer GUI
# ---------------------------------------------------------------------------

class InstallerApp:
    def __init__(self):
        self.root = Tk()
        self.root.title(f"{APP_NAME} v{APP_VERSION} Setup")
        self.root.geometry("560x420")
        self.root.resizable(False, False)
        self.root.configure(bg="#1e1e2e")

        # Try to set icon
        try:
            ico = get_stage_dir() / "resources" / "icons" / "zwg.ico"
            if ico.exists():
                self.root.iconbitmap(str(ico))
        except Exception:
            pass

        default_dir = str(Path(os.environ.get("LOCALAPPDATA", "C:\\Program Files")) / "ZWG Terminal")
        self.install_dir = StringVar(value=default_dir)
        self.create_desktop = BooleanVar(value=True)
        self.create_startmenu = BooleanVar(value=True)

        self.show_welcome()

    def clear(self):
        for w in self.root.winfo_children():
            w.destroy()

    # --- Screens ---

    def show_welcome(self):
        self.clear()
        frm = Frame(self.root, bg="#1e1e2e")
        frm.pack(fill=BOTH, expand=True, padx=30, pady=20)

        Label(frm, text="ZWG Terminal", font=("Segoe UI", 24, "bold"),
              fg="#cdd6f4", bg="#1e1e2e").pack(pady=(20, 5))
        Label(frm, text=f"v{APP_VERSION}", font=("Segoe UI", 12),
              fg="#a6adc8", bg="#1e1e2e").pack()
        Label(frm, text="Ghostty VT + GPUI + ConPTY\n高速 Windows ターミナルエミュレータ",
              font=("Segoe UI", 10), fg="#bac2de", bg="#1e1e2e",
              justify=CENTER).pack(pady=(20, 30))

        btn_frm = Frame(frm, bg="#1e1e2e")
        btn_frm.pack(side=BOTTOM, fill=X)
        Button(btn_frm, text="次へ >", command=self.show_options,
               font=("Segoe UI", 10), bg="#0a84ff", fg="white",
               relief=FLAT, padx=20, pady=6).pack(side=RIGHT)
        Button(btn_frm, text="キャンセル", command=self.root.destroy,
               font=("Segoe UI", 10), bg="#45475a", fg="#cdd6f4",
               relief=FLAT, padx=20, pady=6).pack(side=RIGHT, padx=(0, 10))

    def show_options(self):
        self.clear()
        frm = Frame(self.root, bg="#1e1e2e")
        frm.pack(fill=BOTH, expand=True, padx=30, pady=20)

        Label(frm, text="インストール設定", font=("Segoe UI", 16, "bold"),
              fg="#cdd6f4", bg="#1e1e2e").pack(anchor=W, pady=(0, 15))

        # Install directory
        Label(frm, text="インストール先:", font=("Segoe UI", 10),
              fg="#bac2de", bg="#1e1e2e").pack(anchor=W)
        dir_frm = Frame(frm, bg="#1e1e2e")
        dir_frm.pack(fill=X, pady=(2, 12))
        Entry(dir_frm, textvariable=self.install_dir, font=("Consolas", 9),
              bg="#313244", fg="#cdd6f4", insertbackground="#cdd6f4",
              relief=FLAT).pack(side=LEFT, fill=X, expand=True, ipady=4)
        Button(dir_frm, text="参照...", command=self.browse_dir,
               font=("Segoe UI", 9), bg="#45475a", fg="#cdd6f4",
               relief=FLAT, padx=10).pack(side=RIGHT, padx=(6, 0))

        # Shortcuts
        Checkbutton(frm, text="デスクトップにショートカットを作成",
                    variable=self.create_desktop, font=("Segoe UI", 10),
                    fg="#bac2de", bg="#1e1e2e", selectcolor="#313244",
                    activebackground="#1e1e2e").pack(anchor=W, pady=2)
        Checkbutton(frm, text="スタートメニューに追加",
                    variable=self.create_startmenu, font=("Segoe UI", 10),
                    fg="#bac2de", bg="#1e1e2e", selectcolor="#313244",
                    activebackground="#1e1e2e").pack(anchor=W, pady=2)

        # Buttons
        btn_frm = Frame(frm, bg="#1e1e2e")
        btn_frm.pack(side=BOTTOM, fill=X)
        Button(btn_frm, text="インストール", command=self.start_install,
               font=("Segoe UI", 10, "bold"), bg="#0a84ff", fg="white",
               relief=FLAT, padx=20, pady=6).pack(side=RIGHT)
        Button(btn_frm, text="< 戻る", command=self.show_welcome,
               font=("Segoe UI", 10), bg="#45475a", fg="#cdd6f4",
               relief=FLAT, padx=20, pady=6).pack(side=RIGHT, padx=(0, 10))

    def browse_dir(self):
        d = filedialog.askdirectory(title="インストール先を選択")
        if d:
            self.install_dir.set(d)

    def start_install(self):
        self.clear()
        frm = Frame(self.root, bg="#1e1e2e")
        frm.pack(fill=BOTH, expand=True, padx=30, pady=20)

        Label(frm, text="インストール中...", font=("Segoe UI", 16, "bold"),
              fg="#cdd6f4", bg="#1e1e2e").pack(anchor=W, pady=(0, 15))

        self.progress = Progressbar(frm, length=480, mode="determinate")
        self.progress.pack(pady=(10, 10))

        self.log_text = Text(frm, height=10, font=("Consolas", 9),
                             bg="#181825", fg="#a6adc8", relief=FLAT,
                             state=DISABLED)
        self.log_text.pack(fill=BOTH, expand=True, pady=(5, 0))

        threading.Thread(target=self.do_install, daemon=True).start()

    def log(self, msg: str):
        self.log_text.configure(state=NORMAL)
        self.log_text.insert(END, msg + "\n")
        self.log_text.see(END)
        self.log_text.configure(state=DISABLED)

    def set_progress(self, value: int):
        self.progress["value"] = value
        self.root.update_idletasks()

    def do_install(self):
        try:
            stage = get_stage_dir()
            dest = Path(self.install_dir.get())

            # Step 1: Create directory
            self.log(f"ディレクトリ作成: {dest}")
            dest.mkdir(parents=True, exist_ok=True)
            self.set_progress(10)

            # Step 2: Copy main executable
            self.log("zwg.exe をコピー中...")
            shutil.copy2(stage / "zwg.exe", dest / "zwg.exe")
            self.set_progress(30)

            # Step 3: Copy resources
            self.log("リソースをコピー中...")
            dest_res = dest / "resources"
            if dest_res.exists():
                shutil.rmtree(dest_res)
            shutil.copytree(stage / "resources", dest_res)
            self.set_progress(50)

            # Step 4: Create uninstaller
            self.log("アンインストーラーを作成中...")
            uninstaller_src = stage / "zwg_uninstaller.py"
            uninstaller_bat = dest / "uninstall.bat"
            if uninstaller_src.exists():
                shutil.copy2(uninstaller_src, dest / "zwg_uninstaller.py")
                uninstaller_bat.write_text(
                    f'@echo off\npython "{dest / "zwg_uninstaller.py"}" "{dest}"\n',
                    encoding="utf-8",
                )
            self.set_progress(60)

            # Step 5: Desktop shortcut
            ico = str(dest / "resources" / "icons" / "zwg.ico")
            if self.create_desktop.get():
                self.log("デスクトップショートカットを作成中...")
                desktop = Path(os.path.expanduser("~/Desktop"))
                create_shortcut(
                    str(desktop / "ZWG Terminal.lnk"),
                    str(dest / "zwg.exe"),
                    ico,
                    str(dest),
                )
            self.set_progress(75)

            # Step 6: Start Menu
            if self.create_startmenu.get():
                self.log("スタートメニューに追加中...")
                start_menu = Path(os.environ.get("APPDATA", "")) / "Microsoft" / "Windows" / "Start Menu" / "Programs"
                sm_dir = start_menu / "ZWG Terminal"
                sm_dir.mkdir(parents=True, exist_ok=True)
                create_shortcut(
                    str(sm_dir / "ZWG Terminal.lnk"),
                    str(dest / "zwg.exe"),
                    ico,
                    str(dest),
                )
                if uninstaller_bat.exists():
                    create_shortcut(
                        str(sm_dir / "Uninstall ZWG Terminal.lnk"),
                        str(uninstaller_bat),
                        ico,
                        str(dest),
                    )
            self.set_progress(90)

            # Step 7: Register in Add/Remove Programs
            self.log("レジストリに登録中...")
            register_uninstall(dest, uninstaller_bat)
            self.set_progress(100)

            self.log("\nインストール完了!")
            self.root.after(0, self.show_complete)

        except Exception as e:
            self.log(f"\nエラー: {e}")
            self.root.after(0, lambda: messagebox.showerror("エラー", str(e)))

    def show_complete(self):
        self.clear()
        frm = Frame(self.root, bg="#1e1e2e")
        frm.pack(fill=BOTH, expand=True, padx=30, pady=20)

        Label(frm, text="インストール完了", font=("Segoe UI", 20, "bold"),
              fg="#a6e3a1", bg="#1e1e2e").pack(pady=(40, 10))
        Label(frm, text=f"{APP_NAME} v{APP_VERSION} のインストールが\n正常に完了しました。",
              font=("Segoe UI", 11), fg="#bac2de", bg="#1e1e2e",
              justify=CENTER).pack(pady=10)
        Label(frm, text=f"インストール先: {self.install_dir.get()}",
              font=("Consolas", 9), fg="#a6adc8", bg="#1e1e2e").pack(pady=5)

        btn_frm = Frame(frm, bg="#1e1e2e")
        btn_frm.pack(side=BOTTOM, fill=X)

        self.launch_var = BooleanVar(value=True)
        Checkbutton(btn_frm, text="ZWG Terminal を起動",
                    variable=self.launch_var, font=("Segoe UI", 10),
                    fg="#bac2de", bg="#1e1e2e", selectcolor="#313244",
                    activebackground="#1e1e2e").pack(side=LEFT)

        Button(btn_frm, text="完了", command=self.finish,
               font=("Segoe UI", 10, "bold"), bg="#0a84ff", fg="white",
               relief=FLAT, padx=20, pady=6).pack(side=RIGHT)

    def finish(self):
        if self.launch_var.get():
            exe = Path(self.install_dir.get()) / "zwg.exe"
            if exe.exists():
                subprocess.Popen([str(exe)], cwd=str(exe.parent))
        self.root.destroy()

    def run(self):
        self.root.mainloop()


if __name__ == "__main__":
    app = InstallerApp()
    app.run()
