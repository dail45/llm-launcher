# Builds Windows binaries inside Docker (Docker Desktop with WSL2 backend is enough).
# Usage:  .\build-windows.ps1         -> artifacts in .\dist
$ErrorActionPreference = 'Stop'
$env:DOCKER_BUILDKIT = '1'
$sha = (git rev-parse --short=7 HEAD 2>$null)
docker build --target export --build-arg "LLL_GIT_SHA=$sha" --output type=local,dest=dist .
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
Get-ChildItem dist
