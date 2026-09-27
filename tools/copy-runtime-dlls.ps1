param([Parameter(Mandatory = $true)][string]$DistDirectory)
$ErrorActionPreference = 'Stop'
# Ship Microsoft's redistributable CRT beside the entry points and providers.
$redist = $env:VCToolsRedistDir
if (-not $redist) {
    $directory = (Get-Item (Get-Command lib.exe -ErrorAction Stop).Source).Directory
    while ($directory -and $directory.Name -ne 'VC') { $directory = $directory.Parent }
    if (-not $directory) { throw 'Cannot locate VC redistributables; set VCToolsRedistDir.' }
    $redist = Get-ChildItem -LiteralPath (Join-Path $directory.FullName 'Redist/MSVC') -Directory |
        Where-Object Name -Match '^\d+\.\d+\.\d+$' |
        Sort-Object { [version]$_.Name } -Descending | Select-Object -First 1 -ExpandProperty FullName
}
$crt = Get-ChildItem -LiteralPath (Join-Path $redist 'x64') -Directory -Filter 'Microsoft.VC*.CRT' |
    Select-Object -First 1
if (-not $crt) { throw 'The x64 VC redistributable CRT directory is missing.' }
foreach ($destination in @($DistDirectory, (Join-Path $DistDirectory 'rootfs/lib'))) {
    New-Item -ItemType Directory -Path $destination -Force | Out-Null
    Get-ChildItem -LiteralPath $crt.FullName -Filter '*.dll' | ForEach-Object {
        Copy-Item -LiteralPath $_.FullName -Destination $destination -Force
    }
}
