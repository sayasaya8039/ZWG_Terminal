# -*- mode: python ; coding: utf-8 -*-


a = Analysis(
    ['D:\\NEXTCLOUD\\Windows_app\\ZWG_Terminal\\installer\\zwg_installer.py'],
    pathex=[],
    binaries=[],
    datas=[('D:\\NEXTCLOUD\\Windows_app\\ZWG_Terminal\\installer\\_stage', '_stage')],
    hiddenimports=[],
    hookspath=[],
    hooksconfig={},
    runtime_hooks=[],
    excludes=[],
    noarchive=False,
    optimize=0,
)
pyz = PYZ(a.pure)

exe = EXE(
    pyz,
    a.scripts,
    a.binaries,
    a.datas,
    [],
    name='ZWG_Terminal_Setup',
    debug=False,
    bootloader_ignore_signals=False,
    strip=False,
    upx=True,
    upx_exclude=[],
    runtime_tmpdir=None,
    console=False,
    disable_windowed_traceback=False,
    argv_emulation=False,
    target_arch=None,
    codesign_identity=None,
    entitlements_file=None,
    icon=['D:\\NEXTCLOUD\\Windows_app\\ZWG_Terminal\\resources\\icons\\zwg.ico'],
)
