@echo off
chcp 65001 >nul
setlocal enabledelayedexpansion

echo Installing dependencies...
pip install -r requirements.txt
if %errorlevel% neq 0 (
    echo pip install failed
    exit /b %errorlevel%
)

echo.
echo Building with PyInstaller...
pyinstaller --onefile --windowed --name "voicen" --hidden-import pyperclip --noconfirm main.py
if %errorlevel% neq 0 (
    echo PyInstaller failed
    exit /b %errorlevel%
)

echo.
echo     Done! EXE at dist\voicen.exe
echo.
echo To run: dist\voicen.exe
