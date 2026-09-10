@echo off
rem Open the gdut-net log directory (config + dial book + logs live under ProgramData).
if not exist "C:\ProgramData\gdut-net\logs" mkdir "C:\ProgramData\gdut-net\logs"
explorer.exe "C:\ProgramData\gdut-net\logs"
