@echo off
python "%~dp0native-link.py" %*
exit /b %errorlevel%
