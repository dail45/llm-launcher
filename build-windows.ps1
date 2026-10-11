# Builds Windows binaries inside Docker (Docker Desktop with WSL2 backend is enough).
# Usage:  .\build-windows.ps1         -> artifacts in .\dist
$ErrorActionPreference = 'Stop'
$env:DOCKER_BUILDKIT = '1'
docker build --target export --output type=local,dest=dist .
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
Get-ChildItem dist
