[CmdletBinding()]
param(
    # Which cargo profile to package: release (default) or debug.
    [string]$Configuration = "release",
    # Skip signing. A local build signs by default so the package installs with
    # `Add-AppxPackage -Path`; CI passes this because the Store re-signs the package
    # and a self-signed certificate would only make the artifact harder to verify.
    [switch]$NoSign,
    # Register the staged folder as a development package. No certificate and no
    # administrator rights; Developer Mode must be enabled in Windows settings.
    [switch]$Register
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

# --- identity -----------------------------------------------------------------
# Values come from the environment; a local .env (gitignored) is read as fallback so
# a developer does not have to re-export them in every shell.
$envFile = Join-Path $root ".env"
if (Test-Path $envFile) {
    Get-Content $envFile | ForEach-Object {
        if ($_ -match '^\s*([A-Za-z_][A-Za-z0-9_]*)\s*=\s*(.*?)\s*$') {
            $name = $Matches[1]
            $value = $Matches[2].Trim('"')
            if (-not [Environment]::GetEnvironmentVariable($name, "Process")) {
                [Environment]::SetEnvironmentVariable($name, $value, "Process")
            }
        }
    }
}
foreach ($name in "MSIX_IDENTITY_NAME", "MSIX_PUBLISHER", "MSIX_PUBLISHER_DISPLAY_NAME") {
    if (-not [Environment]::GetEnvironmentVariable($name, "Process")) {
        throw "$name is not set. Put it in .env or the environment."
    }
}

$version = (Select-String -Path "Cargo.toml" -Pattern '^version\s*=\s*"([^"]+)"').Matches[0].Groups[1].Value
$msixVersion = "$version.0"

# --- build --------------------------------------------------------------------
# The Store flavour gets its own target directory. The default flavour in
# target\release is what the MSI and the ZIP are built from, and building over it
# would silently strip the updater out of those artifacts.
$out = Join-Path $root "target\msix"
$targetDir = Join-Path $out "cargo"
$env:CARGO_TARGET_DIR = $targetDir

# Always a Store build: the `store` feature compiles the GitHub updater out, and the
# default (MSI/ZIP) flavour would offer users a download from outside the Store.
$cargoArgs = @("build")
if ($Configuration -eq "release") { $cargoArgs += "--release" }
$cargoArgs += @("--no-default-features", "--features", "store")
& cargo @cargoArgs
if ($LASTEXITCODE -ne 0) { throw "cargo build failed with $LASTEXITCODE" }

# --- payload ------------------------------------------------------------------
$bin = Join-Path $targetDir $Configuration
if (-not (Test-Path (Join-Path $bin "WinGlide.exe"))) {
    throw "cargo reported success but $bin\WinGlide.exe is missing"
}

$stage = Join-Path $out "stage"
Remove-Item $stage -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Path $stage -Force | Out-Null

# Same set as the MSI's dist/: the exe, the self-contained Windows App SDK runtime,
# its resources and the language folders.
Copy-Item (Join-Path $bin "WinGlide.exe") $stage
Get-ChildItem $bin -File | Where-Object { $_.Extension -in ".dll", ".pri" } | Copy-Item -Destination $stage
Copy-Item (Join-Path $bin "Microsoft.UI.Xaml") $stage -Recurse
Get-ChildItem $bin -Directory | Where-Object { $_.Name -match '^[a-z]{2}(-[A-Za-z]+)*$' } | Copy-Item -Destination $stage -Recurse
Copy-Item (Join-Path $PSScriptRoot "Assets") $stage -Recurse

$manifest = Get-Content (Join-Path $PSScriptRoot "AppxManifest.xml") -Raw
$manifest = $manifest.Replace("{{IDENTITY_NAME}}", $env:MSIX_IDENTITY_NAME).Replace("{{PUBLISHER}}", $env:MSIX_PUBLISHER).Replace("{{PUBLISHER_DISPLAY_NAME}}", $env:MSIX_PUBLISHER_DISPLAY_NAME).Replace("{{VERSION}}", $msixVersion)
Set-Content (Join-Path $stage "AppxManifest.xml") $manifest -Encoding UTF8

# --- resource index -----------------------------------------------------------
# Windows resolves `*.targetsize-<n>_altform-unplated.png` through resources.pri; the
# file names alone are not enough, which is why the taskbar kept painting the plate.
# Indexing the whole stage would duplicate every entry that already exists in the
# Windows App SDK's own PRI, so the index is built from a small root containing only
# Assets/ and merged with the SDK PRI instead.
$sdkBin = "C:\Program Files (x86)\Windows Kits\10\bin"
$makepri = Get-ChildItem $sdkBin -Recurse -Filter makepri.exe -ErrorAction SilentlyContinue |
    Where-Object { $_.FullName -match '\\x64\\' } |
    Sort-Object FullName -Descending | Select-Object -First 1
if (-not $makepri) { throw "makepri.exe not found under $sdkBin; install the Windows SDK" }

$priRoot = Join-Path $out "pri-root"
Remove-Item $priRoot -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Path $priRoot | Out-Null
Copy-Item (Join-Path $stage "Assets") $priRoot -Recurse

$sdkPri = Join-Path $stage "resources.pri"
$priConfig = Join-Path $priRoot "priconfig.xml"
@"
<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<resources targetOsVersion="10.0.0" majorVersion="1">
  <index startIndexAt="$sdkPri" root="\">
    <indexer-config type="PRI"/>
  </index>
  <index startIndexAt="\" root="\">
    <default>
      <qualifier name="Language" value="en-US"/>
      <qualifier name="Scale" value="100"/>
      <qualifier name="TargetSize" value="256"/>
      <qualifier name="AlternateForm" value=""/>
    </default>
    <indexer-config type="folder" foldernameAsQualifier="true" filenameAsQualifier="true" qualifierDelimiter="."/>
  </index>
</resources>
"@ | Set-Content $priConfig -Encoding UTF8

& $makepri.FullName new /pr $priRoot /cf $priConfig /of (Join-Path $priRoot "resources.pri") /o
if ($LASTEXITCODE -ne 0) { throw "makepri failed with $LASTEXITCODE" }
Move-Item (Join-Path $priRoot "resources.pri") $sdkPri -Force
Remove-Item $priRoot -Recurse -Force
Write-Host "resource index merged into resources.pri"

# --- pack ---------------------------------------------------------------------
$makeappx = Get-ChildItem $sdkBin -Recurse -Filter makeappx.exe -ErrorAction SilentlyContinue |
    Where-Object { $_.FullName -match '\\x64\\' } |
    Sort-Object FullName -Descending | Select-Object -First 1
if (-not $makeappx) { throw "makeappx.exe not found under $sdkBin; install the Windows SDK" }

$msix = Join-Path $out "WinGlide_${version}_x64.msix"
& $makeappx.FullName pack /d $stage /p $msix /o
if ($LASTEXITCODE -ne 0) { throw "makeappx failed with $LASTEXITCODE" }
Write-Host "packed: $msix"

# --- register -----------------------------------------------------------------
if ($Register) {
    Add-AppxPackage -Register (Join-Path $stage "AppxManifest.xml")
    Write-Host "registered as a development package: $env:MSIX_IDENTITY_NAME"
}

# --- sign ---------------------------------------------------------------------
# Signed by default: `Add-AppxPackage -Path` refuses an unsigned package with
# 0x800B0100. Windows also has to trust the certificate itself, which for a
# self-signed one means LocalMachine\TrustedPeople (administrator) - see README.
if (-not $NoSign) {
    $certPath = Join-Path $out "dev-cert.pfx"
    $cerPath = Join-Path $out "dev-cert.cer"
    $password = "winglide-dev"
    if (-not (Test-Path $certPath)) {
        $cert = New-SelfSignedCertificate -Type Custom -Subject $env:MSIX_PUBLISHER `
            -KeyUsage DigitalSignature -FriendlyName "WinGlide dev signing" `
            -CertStoreLocation "Cert:\CurrentUser\My" -KeyExportPolicy Exportable `
            -TextExtension @("2.5.29.37={text}1.3.6.1.5.5.7.3.3", "2.5.29.19={text}")
        $secure = ConvertTo-SecureString -String $password -Force -AsPlainText
        Export-PfxCertificate -Cert $cert -FilePath $certPath -Password $secure | Out-Null
        Export-Certificate -Cert $cert -FilePath $cerPath | Out-Null
        Write-Host "created dev certificate in Cert:\CurrentUser\My"
    }

    $signtool = Get-ChildItem $sdkBin -Recurse -Filter signtool.exe -ErrorAction SilentlyContinue |
        Where-Object { $_.FullName -match '\\x64\\' } |
        Sort-Object FullName -Descending | Select-Object -First 1
    if (-not $signtool) { throw "signtool.exe not found under $sdkBin" }

    & $signtool.FullName sign /fd SHA256 /f $certPath /p $password $msix
    if ($LASTEXITCODE -ne 0) { throw "signtool failed with $LASTEXITCODE" }
    Write-Host "signed: $msix"
} else {
    Write-Host "unsigned (0x800B0100): $msix"
}
