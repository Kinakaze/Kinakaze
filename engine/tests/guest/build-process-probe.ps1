param(
    [string]$LinkDirectory = (Join-Path $PSScriptRoot '../../../target/debug/elf-imports'),
    [string]$OutputDirectory = (Join-Path $PSScriptRoot '../../../artifacts')
)
$ErrorActionPreference = 'Stop'
New-Item -ItemType Directory -Path $OutputDirectory -Force | Out-Null
$source = Join-Path $PSScriptRoot 'process_probe.c'
$object = Join-Path $OutputDirectory 'process_probe.o'
$executable = Join-Path $OutputDirectory 'process_probe'
& clang --target=x86_64-linux-gnu -ffreestanding -fno-stack-protector -fno-builtin -fPIC -O1 -c $source -o $object
if ($LASTEXITCODE -ne 0) { throw 'Linux process probe compilation failed' }
& ld.lld -pie --dynamic-linker /lib64/ld-linux-x86-64.so.2 -e _start $object -L $LinkDirectory '-l:libc.so.6' -o $executable
if ($LASTEXITCODE -ne 0) { throw 'Linux process probe linking failed' }
Write-Output $executable
