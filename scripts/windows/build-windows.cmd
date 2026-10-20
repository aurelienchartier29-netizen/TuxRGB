@echo off
rem TuxRGB for Windows: double-click to build tuxrgb.exe and the
rem installer (dist\TuxRGB-<version>-Windows-Setup.exe). See build-windows.ps1.
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0build-windows.ps1" %*
pause
