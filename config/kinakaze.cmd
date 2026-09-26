@echo off
"%~dp0worker.exe" %*
exit /b %errorlevel%
