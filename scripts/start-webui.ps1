[CmdletBinding()]
param(
    [ValidateRange(1, 65535)][int]$Port = 7878,
    [switch]$Rebuild,
    [switch]$NoBrowser,
    [switch]$Check,
    [switch]$CpuRender
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Set-Location -LiteralPath $root
$url = "http://127.0.0.1:$Port"
$browserJob = $null
if ($CpuRender) { $env:IMAGEPICKER_RENDER = 'cpu' }
# Disable injected Vulkan overlays only in this process and its children.
if (-not $env:VK_LOADER_LAYERS_DISABLE) { $env:VK_LOADER_LAYERS_DISABLE = '~implicit~' }
# Reuse the installed development environment without uv synchronizing away CUDA extras.
$workerPython = Join-Path $root 'ai-worker\.venv\Scripts\python.exe'
if (-not $env:IMAGEPICKER_WORKER_CMD -and -not $env:IMAGEPICKER_WORKER_DIR -and
    (Test-Path -LiteralPath $workerPython)) {
    $env:IMAGEPICKER_WORKER_CMD = '"' + $workerPython + '" -m imagepicker_ai'
}

function Test-WebUI {
    try {
        $health = Invoke-RestMethod "$url/api/health" -TimeoutSec 2
        $page = Invoke-WebRequest $url -UseBasicParsing -TimeoutSec 2
        return ($health.ok -eq $true -and $health.version -and $page.Content -match '<title>imagePicker</title>')
    } catch { return $false }
}

try {
    if (Test-WebUI) {
        Write-Host "imagePicker is already running: $url"
        if (-not $NoBrowser -and -not $Check) { Start-Process $url }
        exit 0
    }
    # Fail early rather than build or open an unrelated service on this port.
    $probe = [System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Loopback, $Port)
    try { $probe.Start() } catch { throw "Port $Port is in use. Run Start-WebUI.cmd -Port 7879 or stop the other service." }
    finally { $probe.Stop() }

    $webDir = Join-Path $root 'web\dist'
    if ($Rebuild -or -not (Test-Path -LiteralPath (Join-Path $webDir 'index.html'))) {
        if (-not (Get-Command pnpm.cmd -ErrorAction SilentlyContinue)) {
            throw 'Building WebUI requires Node.js 24 and pnpm 10. Install them and retry.'
        }
        Push-Location (Join-Path $root 'web')
        try {
            & pnpm.cmd install --frozen-lockfile
            if ($LASTEXITCODE -ne 0) { throw 'pnpm install failed.' }
            & pnpm.cmd build
            if ($LASTEXITCODE -ne 0) { throw 'WebUI build failed.' }
        } finally { Pop-Location }
    }

    $exe = @('target\release\imagepicker.exe', 'target\debug\imagepicker.exe') |
        ForEach-Object { Join-Path $root $_ } |
        Where-Object { Test-Path -LiteralPath $_ } | Select-Object -First 1
    if ($Rebuild -or -not $exe) {
        if (-not (Get-Command cargo.exe -ErrorAction SilentlyContinue)) {
            throw 'Building the server requires Rust and Visual Studio C++ Build Tools. Install them and retry.'
        }
        & cargo.exe build --locked --release -p ip-cli --target-dir (Join-Path $root 'target')
        if ($LASTEXITCODE -ne 0) { throw 'Server build failed.' }
        $exe = Join-Path $root 'target\release\imagepicker.exe'
    }

    if ($Check) {
        & $exe --version
        if ($LASTEXITCODE -ne 0) { throw 'Server executable check failed.' }
        Write-Host "Startup files ready: $webDir"
        exit 0
    }

    Write-Host "Starting imagePicker: $url"
    Write-Host 'Keep this window open. Press Ctrl+C or close it to stop the server.'
    if (-not $NoBrowser) {
        # Run the server in this console so closing it also terminates the server.
        $browserJob = Start-Job -ArgumentList $url -ScriptBlock {
            param($address)
            $deadline = (Get-Date).AddSeconds(90)
            while ((Get-Date) -lt $deadline) {
                try {
                    $health = Invoke-RestMethod "$address/api/health" -TimeoutSec 2
                    $page = Invoke-WebRequest $address -UseBasicParsing -TimeoutSec 2
                    if ($health.ok -eq $true -and $page.Content -match '<title>imagePicker</title>') {
                        Start-Process $address
                        return
                    }
                } catch { }
                Start-Sleep -Milliseconds 500
            }
            Write-Warning "Browser auto-open timed out. Open $address manually once the server is ready."
        }
    }
    & $exe serve --host 127.0.0.1 --port $Port --web-dir $webDir
    if ($LASTEXITCODE -ne 0) { throw "Server exited with code $LASTEXITCODE." }
} catch {
    Write-Host "ERROR: $($_.Exception.Message)" -ForegroundColor Red
    exit 1
} finally {
    if ($browserJob) {
        Stop-Job $browserJob
        Receive-Job $browserJob
        Remove-Job $browserJob
    }
}
