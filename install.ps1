# Installs a prebuilt Burdin compiler from a GitHub release on Windows (issues #452-#455).
#
# Install the latest stable release:
#   irm https://raw.githubusercontent.com/RicardoMilos7/burdin/main/install.ps1 | iex
#
# Environment (or the matching parameters when the script is run from a file):
#   BURDIN_VERSION       version to install (default: the latest release of the channel)
#   BURDIN_CHANNEL       stable (default) or nightly
#   BURDIN_INSTALL_DIR   destination directory (default: %LOCALAPPDATA%\Programs\burdin\bin)
#   BURDIN_TARGET        target triple override (default: detected from the OS architecture)
#   BURDIN_BASE_URL      directory that holds the archive and SHA256SUMS
#   BURDIN_RELEASE_DIR   local directory to install from, instead of the network (a rehearsal)
#   BURDIN_NO_MODIFY_PATH  set to any value to leave the user's PATH alone (-NoModifyPath)
#
# Every download is verified against the release's SHA256SUMS before it is unpacked. The compiler needs LLVM-C.dll at
# run time; the archive carries it and this script installs it beside burdin.exe. It warns when the installed compiler
# cannot start anyway (docs/releasing.md). Unless told not to, it appends the install directory to the user's PATH
# (HKCU\Environment), so new terminals find burdin; the current session is updated too.
param(
    [string]$Version = $env:BURDIN_VERSION,
    [string]$Channel = $(if ($env:BURDIN_CHANNEL) { $env:BURDIN_CHANNEL } else { 'stable' }),
    [string]$InstallDir = $env:BURDIN_INSTALL_DIR,
    [string]$Target = $env:BURDIN_TARGET,
    [string]$BaseUrl = $env:BURDIN_BASE_URL,
    [string]$ReleaseDir = $env:BURDIN_RELEASE_DIR,
    [switch]$NoModifyPath
)

$ErrorActionPreference = 'Stop'
$Repo = 'RicardoMilos7/burdin'

function Say([string]$Message) { Write-Host $Message }
function Warn([string]$Message) { Write-Warning $Message }
function Fail([string]$Message) { throw $Message }

# Appends $Directory to the user's PATH unless an entry already names it. The value is read and written as the raw
# registry string, so entries such as %LOCALAPPDATA% stay unexpanded. Returns $true when it changed the PATH.
function Add-ToUserPath([string]$Directory) {
    $wanted = $Directory.TrimEnd('\')
    $key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Environment', $true)
    try {
        $current = [string]$key.GetValue('Path', '', [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
        $entries = @($current -split ';' | Where-Object { $_ -ne '' })
        foreach ($entry in $entries) {
            if ($entry.TrimEnd('\') -ieq $wanted) { return $false }
        }
        $updated = (@($entries) + $Directory) -join ';'
        $key.SetValue('Path', $updated, [Microsoft.Win32.RegistryValueKind]::ExpandString)
        return $true
    } finally {
        $key.Close()
    }
}

if ($Channel -notin @('stable', 'nightly')) {
    Fail "the channel must be stable or nightly, not: $Channel"
}

if (-not $Target) {
    $architecture = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString()
    switch ($architecture) {
        'X64' { $Target = 'x86_64-pc-windows-msvc' }
        'Arm64' { $Target = 'aarch64-pc-windows-msvc' }
        default { Fail "unsupported Windows architecture: $architecture" }
    }
}
if ($Target -notin @('x86_64-pc-windows-msvc', 'aarch64-pc-windows-msvc')) {
    Fail "unsupported target on Windows: $Target (expected x86_64-pc-windows-msvc or aarch64-pc-windows-msvc)"
}

function Get-LatestVersion([string]$ChannelName) {
    $headers = @{ 'User-Agent' = 'burdin-install' }
    $api = "https://api.github.com/repos/$Repo/releases"
    if ($ChannelName -eq 'stable') {
        $release = Invoke-RestMethod -Uri "$api/latest" -Headers $headers
        return $release.tag_name -replace '^v', ''
    }
    $releases = Invoke-RestMethod -Uri "$api`?per_page=30" -Headers $headers
    foreach ($release in $releases) {
        $tag = $release.tag_name -replace '^v', ''
        if ($tag -like '*nightly*') { return $tag }
    }
    Fail 'no nightly release was found; set BURDIN_VERSION'
}

if (-not $Version -and $ReleaseDir) {
    $versionFile = Join-Path $ReleaseDir 'VERSION'
    if (Test-Path $versionFile) { $Version = (Get-Content -Raw $versionFile).Trim() }
}
if (-not $Version) {
    if ($ReleaseDir) { Fail 'set BURDIN_VERSION when installing from BURDIN_RELEASE_DIR' }
    $Version = Get-LatestVersion $Channel
}
if ($Version -notmatch '^[0-9]+\.[0-9]+\.[0-9]+([-+][0-9A-Za-z.-]+)?$') {
    Fail "not a MAJOR.MINOR.PATCH version: $Version"
}

$archive = "burdin-$Version-$Target.tar.gz"
if ($ReleaseDir) {
    $base = $ReleaseDir
} elseif ($BaseUrl) {
    $base = $BaseUrl.TrimEnd('/')
} else {
    $base = "https://github.com/$Repo/releases/download/v$Version"
}

$tmp = Join-Path ([System.IO.Path]::GetTempPath()) ("burdin-install-" + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $tmp -Force | Out-Null

function Fetch([string]$Name, [string]$Destination) {
    if ($ReleaseDir) {
        Copy-Item -LiteralPath (Join-Path $base $Name) -Destination $Destination
    } else {
        Invoke-WebRequest -UseBasicParsing -Uri "$base/$Name" -OutFile $Destination
    }
}

try {
    Say "downloading burdin $Version ($Channel) for $Target"
    $sums = Join-Path $tmp 'SHA256SUMS'
    Fetch 'SHA256SUMS' $sums
    $downloaded = Join-Path $tmp $archive
    Fetch $archive $downloaded

    $expected = $null
    foreach ($line in Get-Content $sums) {
        if ($line -match '^([0-9a-fA-F]{64})\s+\*?(.+)$' -and $Matches[2].Trim() -eq $archive) {
            $expected = $Matches[1].ToLowerInvariant()
            break
        }
    }
    if (-not $expected) { Fail "SHA256SUMS does not list $archive" }
    $actual = (Get-FileHash -Algorithm SHA256 -LiteralPath $downloaded).Hash.ToLowerInvariant()
    if ($expected -ne $actual) {
        Fail "checksum mismatch for $archive`: expected $expected, got $actual"
    }

    $unpacked = Join-Path $tmp 'unpacked'
    New-Item -ItemType Directory -Path $unpacked -Force | Out-Null
    # Use Windows' bsdtar. Git's GNU tar can precede it on PATH (for example when this runs from Git Bash) and reads
    # `C:\...` as a remote archive name.
    $tar = Join-Path $env:SystemRoot 'System32/tar.exe'
    if (-not (Test-Path $tar)) { $tar = 'tar.exe' }
    & $tar -xzf $downloaded -C $unpacked
    if ($LASTEXITCODE -ne 0) { Fail "cannot unpack $archive" }
    $source = Join-Path $unpacked "burdin-$Version-$Target/burdin.exe"
    if (-not (Test-Path $source)) { Fail "the archive does not contain burdin-$Version-$Target/burdin.exe" }

    if (-not $InstallDir) {
        $InstallDir = Join-Path $env:LOCALAPPDATA 'Programs/burdin/bin'
    }
    New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
    $destination = Join-Path $InstallDir 'burdin.exe'
    Copy-Item -LiteralPath $source -Destination $destination -Force
    # The runtime library travels with the compiler: Windows finds a DLL next to the executable first.
    $dll = Join-Path (Split-Path -Parent $source) 'LLVM-C.dll'
    if (Test-Path $dll) {
        Copy-Item -LiteralPath $dll -Destination (Join-Path $InstallDir 'LLVM-C.dll') -Force
    }

    Say "installed burdin $Version ($Channel) for $Target in $InstallDir"
    try {
        & $destination --version 2>&1 | Out-Null
        if ($LASTEXITCODE -eq 0) {
            Say 'checksum verified; the compiler runs'
        } else {
            Warn "$destination did not start. Burdin needs LLVM-C.dll at run time; put it on PATH or beside burdin.exe."
        }
    } catch {
        Warn "$destination did not start. Burdin needs LLVM-C.dll at run time; put it on PATH or beside burdin.exe."
    }
    # Linking needs the MSVC build tools and a Windows SDK; burdin finds them itself, so no Developer Command Prompt is
    # needed. Only say so when no installation with the C++ tools is there.
    $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio/Installer/vswhere.exe'
    $vs = $null
    if (Test-Path $vswhere) {
        $vs = & $vswhere -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
    }
    if (-not $vs) {
        Warn 'burdin links programs with the MSVC build tools, and none were found. Install Build Tools for Visual Studio'
        Warn '(https://visualstudio.microsoft.com/visual-cpp-build-tools/) with the "Desktop development with C++" workload.'
    }
    $onPath = ($env:PATH -split ';') -contains $InstallDir
    $modifyPath = -not ($NoModifyPath -or $env:BURDIN_NO_MODIFY_PATH)
    if ($modifyPath) {
        if (Add-ToUserPath $InstallDir) {
            Say "added $InstallDir to your user PATH; open a new terminal to run burdin from anywhere"
        } else {
            Say "$InstallDir is already on your user PATH"
        }
        if (-not $onPath) { $env:PATH = $env:PATH.TrimEnd(';') + ';' + $InstallDir }
    } elseif (-not $onPath) {
        Say "add $InstallDir to PATH to run burdin from anywhere"
    }
} finally {
    Remove-Item -Recurse -Force -LiteralPath $tmp -ErrorAction SilentlyContinue
}
