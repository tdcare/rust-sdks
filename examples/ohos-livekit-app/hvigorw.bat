@echo off
rem HarmonyOS Hvigor Wrapper Script for Windows

setlocal

rem Hvigor version
set HVIGOR_VERSION=5.0.0

rem Project directory
set APP_HOME=%~dp0
set APP_HOME=%APP_HOME:~0,-1%

rem Hvigor wrapper directory
set HVIGOR_WRAPPER_DIR=%APP_HOME%\hvigor

rem Check if Node.js is installed
where node >nul 2>nul
if %errorlevel% neq 0 (
    echo Error: Node.js is not installed or not in PATH
    exit /b 1
)

rem Check if ohpm is available
where ohpm >nul 2>nul
if %errorlevel% neq 0 (
    echo Warning: ohpm is not in PATH, attempting to use local installation
)

rem Navigate to project directory
cd /d "%APP_HOME%" || exit /b 1

rem Check if hvigor is installed locally
if not exist "oh_modules\@ohos\hvigor" (
    if not exist "node_modules\@ohos\hvigor" (
        echo Hvigor not found, installing dependencies...
        where ohpm >nul 2>nul
        if %errorlevel% equ 0 (
            call ohpm install
        ) else (
            echo Error: Cannot install dependencies without ohpm
            exit /b 1
        )
    )
)

rem Find hvigor executable
set HVIGOR_BIN=

if exist "oh_modules\@ohos\hvigor\bin\hvigor.js" (
    set HVIGOR_BIN=oh_modules\@ohos\hvigor\bin\hvigor.js
) else if exist "node_modules\@ohos\hvigor\bin\hvigor.js" (
    set HVIGOR_BIN=node_modules\@ohos\hvigor\bin\hvigor.js
) else (
    echo Error: Cannot find hvigor executable
    echo Searched in:
    echo   - oh_modules\@ohos\hvigor\bin\hvigor.js
    echo   - node_modules\@ohos\hvigor\bin\hvigor.js
    echo Please run 'ohpm install' first
    exit /b 1
)

rem Execute hvigor with all arguments
node "%HVIGOR_BIN%" %*

endlocal
