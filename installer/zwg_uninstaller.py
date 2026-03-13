"""
ZWG Terminal Uninstaller — Removes ZWG Terminal installation.
"""

import os
import shutil
import sys
import winreg
from pathlib import Path
from tkinter import BOTH, BOTTOM, CENTER, FLAT, RIGHT, X, Button, Frame, Label, Tk, messagebox

APP_NAME = "ZWG Terminal"
UNINSTALL_KEY = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\ZWGTerminal"


def remove_shortcuts():
    """Remove desktop and Start Menu shortcuts."""
    desktop = Path(os.path.expanduser("~/Desktop"))
    lnk = desktop / "ZWG Terminal.lnk"
    if lnk.exists():
        lnk.unlink()

    start_menu = Path(os.environ.get("APPDATA", "")) / "Microsoft" / "Windows" / "Start Menu" / "Programs" / "ZWG Terminal"
    if start_menu.exists():
        shutil.rmtree(start_menu, ignore_errors=True)


def remove_registry():
    """Remove Add/Remove Programs entry."""
    try:
        winreg.DeleteKey(winreg.HKEY_CURRENT_USER, UNINSTALL_KEY)
    except OSError:
        pass


def do_uninstall(install_dir: Path):
    remove_shortcuts()
    remove_registry()

    # Remove installation files (schedule self-delete via batch)
    bat = install_dir / "_cleanup.bat"
    bat.write_text(
        f'@echo off\n'
        f'ping 127.0.0.1 -n 2 > nul\n'
        f'rd /s /q "{install_dir}"\n'
        f'del "%~f0"\n',
        encoding="utf-8",
    )

    import subprocess
    subprocess.Popen(
        ["cmd.exe", "/c", str(bat)],
        creationflags=0x08000000,  # CREATE_NO_WINDOW
    )


class UninstallerApp:
    def __init__(self, install_dir: Path):
        self.install_dir = install_dir
        self.root = Tk()
        self.root.title(f"{APP_NAME} アンインストール")
        self.root.geometry("440x240")
        self.root.resizable(False, False)
        self.root.configure(bg="#1e1e2e")

        try:
            ico = install_dir / "resources" / "icons" / "zwg.ico"
            if ico.exists():
                self.root.iconbitmap(str(ico))
        except Exception:
            pass

        frm = Frame(self.root, bg="#1e1e2e")
        frm.pack(fill=BOTH, expand=True, padx=30, pady=20)

        Label(frm, text=f"{APP_NAME} を\nアンインストールしますか？",
              font=("Segoe UI", 14, "bold"), fg="#cdd6f4", bg="#1e1e2e",
              justify=CENTER).pack(pady=(20, 10))
        Label(frm, text=f"インストール先: {install_dir}",
              font=("Consolas", 9), fg="#a6adc8", bg="#1e1e2e").pack(pady=5)

        btn_frm = Frame(frm, bg="#1e1e2e")
        btn_frm.pack(side=BOTTOM, fill=X)
        Button(btn_frm, text="アンインストール", command=self.confirm,
               font=("Segoe UI", 10, "bold"), bg="#f38ba8", fg="white",
               relief=FLAT, padx=20, pady=6).pack(side=RIGHT)
        Button(btn_frm, text="キャンセル", command=self.root.destroy,
               font=("Segoe UI", 10), bg="#45475a", fg="#cdd6f4",
               relief=FLAT, padx=20, pady=6).pack(side=RIGHT, padx=(0, 10))

    def confirm(self):
        if messagebox.askyesno("確認", f"{APP_NAME} を完全に削除しますか？"):
            do_uninstall(self.install_dir)
            messagebox.showinfo("完了", "アンインストールが完了しました。")
            self.root.destroy()

    def run(self):
        self.root.mainloop()


if __name__ == "__main__":
    if len(sys.argv) > 1:
        install_path = Path(sys.argv[1])
    else:
        # Try to read from registry
        try:
            key = winreg.OpenKey(winreg.HKEY_CURRENT_USER, UNINSTALL_KEY)
            install_path = Path(winreg.QueryValueEx(key, "InstallLocation")[0])
            winreg.CloseKey(key)
        except OSError:
            messagebox.showerror("エラー", "インストール先が見つかりません。")
            sys.exit(1)

    if not install_path.exists():
        messagebox.showerror("エラー", f"ディレクトリが存在しません:\n{install_path}")
        sys.exit(1)

    app = UninstallerApp(install_path)
    app.run()
