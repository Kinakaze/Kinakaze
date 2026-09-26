param(
    [switch]$Release,
    [string]$DistDirectory = 'dist',
    [string]$TargetDirectory = 'target',
    [switch]$RefreshExports,
    [switch]$Development,
    [switch]$SkipFormat,
    [switch]$SkipTests
)
$ErrorActionPreference = 'Stop'
$workspacePath = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$profileName = if ($Release) { 'release' } else { 'debug' }
$profileArgs = if ($Release) { @('--release') } else { @() }

function Invoke-Checked {
    param([string]$Program, [string[]]$Arguments)
    & $Program @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "$Program failed with exit code $LASTEXITCODE"
    }
}

Push-Location -LiteralPath $workspacePath
try {
    $buildRoot = [System.IO.Path]::GetFullPath($TargetDirectory)
    $targetArgs = @('--target-dir', $buildRoot)
    # Export checks inspect compiled DLLs. Keep the feature graph identical to
    # the final build so un-hashed DLL names cannot mix incompatible Rust ABIs.
    Invoke-Checked 'cargo' (@('build', '--workspace', '--lib', '--locked', '--features', 'kinakaze-v2-runtime/guest-engine') + $profileArgs + $targetArgs)
    $exportArgs = @('tools/native-exports/generate.py', '--image-dir', (Join-Path $buildRoot $profileName))
    if ($RefreshExports) {
        Invoke-Checked 'python' $exportArgs
    }
    Invoke-Checked 'python' ($exportArgs + @('--check'))
    if (-not $SkipFormat) {
        Invoke-Checked 'cargo' @('fmt', '--all', '--', '--check')
    }
    Invoke-Checked 'cargo' (@('build', '--workspace', '--locked', '--features', 'kinakaze-v2-runtime/guest-engine') + $profileArgs + $targetArgs)
    # Entry executables have only rlib dependencies. Link their own standard
    # library so Windows can start them before rootfs/lib has been opened.
    Invoke-Checked 'cargo' (@('--config', "build.rustflags=['-C','prefer-dynamic=no']",
        'build', '--locked', '--target-dir', (Join-Path $buildRoot 'entry'), '-p', 'kinakaze-v2-init', '-p', 'kinakaze-v2-worker') + $profileArgs)
    $binaryDir = Join-Path $buildRoot $profileName
    $entryDir = Join-Path $buildRoot "entry/$profileName"
    foreach ($entry in @('init.exe', 'worker.exe')) {
        Copy-Item -LiteralPath (Join-Path $entryDir $entry) -Destination $binaryDir -Force
    }
    $toolchainRoot = (& rustc --print sysroot).Trim()
    if ($LASTEXITCODE -ne 0) { throw 'Cannot identify Rust toolchain' }
    Get-ChildItem -LiteralPath (Join-Path $toolchainRoot 'bin') -Filter 'std-*.dll' | ForEach-Object {
        Copy-Item -LiteralPath $_.FullName -Destination $binaryDir -Force
    }
    $packageArgs = @(
        '--dll-dir', $binaryDir, '--dist-dir', $DistDirectory,
        '--link-dir', (Join-Path $binaryDir 'elf-imports')
    )
    if ($Development) { $packageArgs += '--development' }
    Invoke-Checked (Join-Path $binaryDir 'kinakaze-packager.exe') $packageArgs
    if (-not $SkipTests) {
        Invoke-Checked 'python' @('-m', 'unittest', 'discover', '-s', 'tools/native-exports')
        Invoke-Checked 'python' @('-m', 'unittest', 'discover', '-s', 'tools/guest-deps')
        # Tests have their own Cargo feature graph and Rust dylib ABI. Package
        # the DLLs linked by that graph, not the normal worker build above.
        $testArgs = @('test', '--workspace', '--locked', '--features', 'kinakaze-v2-runtime/guest-engine') + $profileArgs + $targetArgs
        Invoke-Checked 'cargo' ($testArgs + @('--no-run'))
        $testBinaryDir = Join-Path $binaryDir 'deps'
        $testDistDir = Join-Path $binaryDir 'test-dist'
        Get-ChildItem -LiteralPath (Join-Path $toolchainRoot 'bin') -Filter 'std-*.dll' | ForEach-Object {
            Copy-Item -LiteralPath $_.FullName -Destination $testBinaryDir -Force
        }
        Invoke-Checked (Join-Path $binaryDir 'kinakaze-packager.exe') @(
            '--dll-dir', $testBinaryDir, '--exe-dir', $entryDir, '--dist-dir', $testDistDir
        )
        $testPathBefore = $env:PATH
        try {
            $env:PATH = (Join-Path $testDistDir 'rootfs/lib') + [IO.Path]::PathSeparator + $testPathBefore
            # Kernel/VFS tests mutate process-wide PID, descriptor and mapping
            # state; each test still exercises its own threads and children.
            Invoke-Checked 'cargo' ($testArgs + @('--', '--test-threads=1'))
        }
        finally {
            $env:PATH = $testPathBefore
        }
        # Test compilation can rebuild the ordinary executable targets using
        # dynamic std. The separately built entry images remain authoritative.
        foreach ($entry in @('init.exe', 'worker.exe')) {
            Copy-Item -LiteralPath (Join-Path $entryDir $entry) -Destination $binaryDir -Force
        }
        Invoke-Checked (Join-Path $entryDir 'worker.exe') @('smoke', '--dist', $DistDirectory)
    }
}
finally {
    Pop-Location
}
