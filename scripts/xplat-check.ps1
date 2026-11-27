# 跨平台类型检查脚本（隔离副本用）
# 用法：pwsh -File scripts/xplat-check.ps1 [-Repo E:\_xplat_ui]
#
# 用「假 C 编译器」让 cc-rs 类 crate（libsqlite3-sys / ring / wayland）跳过 C 编译，
# 从而让 rustc 真正对 linux/macOS 目标做**第一方源码**的类型检查。
param(
    [string]$Repo = "E:\_xplat_ui",
    [string]$Targets = "x86_64-unknown-linux-gnu,aarch64-apple-darwin,x86_64-apple-darwin",
    [string]$Pkgs = "credit-core,credit-gateway,credit-cli,credit-ui"
)

$ErrorActionPreference = "Continue"
$fake = Join-Path $Repo "fakecc"

function Set-TargetEnv([string]$t) {
    $env:CC = "$fake\cc.bat"
    $env:AR = "$fake\ar.bat"
    $env:CXX = "$fake\g++.bat"
    $env:PKG_CONFIG = "$fake\pkg-config.bat"
    $env:PKG_CONFIG_ALLOW_CROSS = "1"
    $env:RUST_FONTCONFIG_DLOPEN = "1"
    $env:FREETYPE2_NO_PKG_CONFIG = "1"
    switch ($t) {
        "x86_64-unknown-linux-gnu" {
            $env:CC_x86_64_unknown_linux_gnu = "$fake\x86_64-linux-gnu-gcc.bat"
            $env:AR_x86_64_unknown_linux_gnu = "$fake\x86_64-linux-gnu-ar.bat"
            $env:CXX_x86_64_unknown_linux_gnu = "$fake\x86_64-linux-gnu-g++.bat"
        }
        "aarch64-apple-darwin" {
            $env:CC_aarch64_apple_darwin = "$fake\aarch64-apple-darwin-gcc.bat"
            $env:AR_aarch64_apple_darwin = "$fake\aarch64-apple-darwin-ar.bat"
        }
        "x86_64-apple-darwin" {
            $env:CC_x86_64_apple_darwin = "$fake\x86_64-apple-darwin-gcc.bat"
            $env:AR_x86_64_apple_darwin = "$fake\x86_64-apple-darwin-ar.bat"
        }
    }
}

$results = @()
foreach ($t in $Targets.Split(",")) {
    foreach ($p in $Pkgs.Split(",")) {
        Write-Host "=== check $p -> $t ===" -ForegroundColor Cyan
        Set-TargetEnv $t
        Push-Location $Repo
        $out = cargo check --target $t -p $p --message-format short 2>&1 | Out-String -Width 250
        Pop-Location
        $errs = ($out -split "`n") | Where-Object { $_ -match "error(\[|:)" -or $_ -match "warning:" } | Where-Object { $_ -notmatch "fake C compiler" }
        if ($errs) {
            Write-Host "  FAIL ($($errs.Count)):" -ForegroundColor Red
            $errs | Select-Object -First 15 | ForEach-Object { Write-Host "    $_" -ForegroundColor Red }
            $results += [pscustomobject]@{ target=$t; pkg=$p; status="FAIL"; detail=($errs -join " | ") }
        } else {
            Write-Host "  OK" -ForegroundColor Green
            $results += [pscustomobject]@{ target=$t; pkg=$p; status="OK"; detail="" }
        }
    }
}

Write-Host "`n=== SUMMARY ===" -ForegroundColor Cyan
$results | Format-Table -AutoSize
if ($results | Where-Object { $_.status -eq "FAIL" }) { exit 1 }
