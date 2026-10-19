@echo off
rem TuxRGB - Windows Beta Build: double-click to build tuxrgb.exe and the
rem installer (dist\TuxRGB-<version>-win-beta-Setup.exe). See build-windows.ps1.
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0build-windows.ps1" %*
pause
