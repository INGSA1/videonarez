<#
  Выпуск обновления Videonarez.

  .\scripts\release.ps1 -Version 0.2.1 -Notes "Что изменилось"

  1. Проставляет версию в tauri.conf.json, Cargo.toml и package.json (если указан -Version).
  2. Собирает установщик NSIS и подписывает его ключом обновлений.
  3. Формирует latest.json и загружает установщик + latest.json на сервер (latest.json — последним,
     чтобы клиенты никогда не увидели версию, файл которой ещё не докачан).
  4. Проверяет, что latest.json отдаётся по HTTPS с новой версией.
  5. Коммитит и пушит смену версии, затем создаёт релиз vX.Y.Z на GitHub на этом коммите
     с установщиком и подписью. Если релиз уже есть — обновляет его файлы. -NoGitHub отключает шаг.

  Длинное описание релиза удобнее передать файлом: -NotesFile notes.md (используется и для latest.json).

  Ключ подписи: %USERPROFILE%\.tauri\videonarez-updater.key (без пароля). Держите его в сохранности:
  без него следующее обновление подписать нельзя, а установленные копии примут только его подпись.
#>
param(
  [string]$Version,
  [string]$Notes = "",
  [string]$SigningKey = "$env:USERPROFILE\.tauri\videonarez-updater.key",
  [string]$SshKey = "$env:USERPROFILE\.ssh\videonarez-server\ssh-key-1793873540012",
  [string]$Server = "ingsal@84.201.181.80",
  [string]$RemoteDir = "/var/www/videonarez-updates",
  [string]$BaseUrl = "https://84-201-181-80.sslip.io/videonarez",
  [string]$NotesFile,
  [switch]$SkipBuild,
  [switch]$NoGitHub
)
$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$utf8 = New-Object System.Text.UTF8Encoding $false
$conf = Join-Path $root "src-tauri\tauri.conf.json"

function Set-Version([string]$v) {
  $files = @(
    @{ Path = $conf; Pattern = '"version":\s*"[^"]+"'; Value = "`"version`": `"$v`"" ; Count = 1 },
    @{ Path = (Join-Path $root "package.json"); Pattern = '"version":\s*"[^"]+"'; Value = "`"version`": `"$v`""; Count = 1 },
    @{ Path = (Join-Path $root "src-tauri\Cargo.toml"); Pattern = '(?m)^version = "[^"]+"'; Value = "version = `"$v`""; Count = 1 }
  )
  foreach ($f in $files) {
    $t = [IO.File]::ReadAllText($f.Path)
    $t = ([regex]$f.Pattern).Replace($t, $f.Value, $f.Count)
    [IO.File]::WriteAllText($f.Path, $t, $utf8)
  }
}

if ($NotesFile) {
  if (-not (Test-Path $NotesFile)) { throw "Нет файла описания: $NotesFile" }
  $Notes = [IO.File]::ReadAllText((Resolve-Path $NotesFile), [Text.Encoding]::UTF8).Trim()
}

if ($Version) { Set-Version $Version }
$Version = ((Get-Content $conf -Raw | ConvertFrom-Json).version)
Write-Host "Версия: $Version"

if (-not (Test-Path $SigningKey)) { throw "Нет ключа подписи: $SigningKey" }

if (-not $SkipBuild) {
  $env:TAURI_SIGNING_PRIVATE_KEY = (Get-Content $SigningKey -Raw).Trim()
  $env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = ""
  Push-Location $root
  try {
    npx tauri build
    if ($LASTEXITCODE -ne 0) { throw "Сборка не удалась" }
  } finally {
    Pop-Location
    Remove-Item Env:TAURI_SIGNING_PRIVATE_KEY -ErrorAction SilentlyContinue
  }
}

$nsis = Join-Path $root "src-tauri\target\release\bundle\nsis"
$exeName = "Videonarez_${Version}_x64-setup.exe"
$exe = Join-Path $nsis $exeName
$sig = "$exe.sig"
if (-not (Test-Path $exe)) { throw "Нет установщика: $exe" }
if (-not (Test-Path $sig)) { throw "Нет подписи: $sig (createUpdaterArtifacts и ключ подписи должны быть заданы)" }

$latest = [ordered]@{
  version  = $Version
  notes    = $Notes
  pub_date = (Get-Date).ToUniversalTime().ToString("yyyy-MM-ddTHH:mm:ssZ")
  platforms = [ordered]@{
    "windows-x86_64" = [ordered]@{
      signature = (Get-Content $sig -Raw).Trim()
      url       = "$BaseUrl/$exeName"
    }
  }
}
$latestPath = Join-Path $nsis "latest.json"
[IO.File]::WriteAllText($latestPath, ($latest | ConvertTo-Json -Depth 5), $utf8)

$sshOpts = @("-i", $SshKey, "-o", "BatchMode=yes")
Write-Host "Загружаю $exeName ($([math]::Round((Get-Item $exe).Length / 1MB, 1)) МБ)…"
scp @sshOpts $exe "${Server}:$RemoteDir/$exeName"
if ($LASTEXITCODE -ne 0) { throw "Не удалось загрузить установщик" }
scp @sshOpts $latestPath "${Server}:$RemoteDir/latest.json"
if ($LASTEXITCODE -ne 0) { throw "Не удалось загрузить latest.json" }

$check = Invoke-RestMethod -Uri "$BaseUrl/latest.json" -Headers @{ "Cache-Control" = "no-cache" }
if ($check.version -ne $Version) { throw "Сервер отдаёт версию $($check.version), ожидалась $Version" }
$head = Invoke-WebRequest -Uri "$BaseUrl/$exeName" -Method Head -UseBasicParsing
Write-Host "Опубликовано: версия $($check.version), установщик $([math]::Round([int64]$head.Headers['Content-Length'] / 1MB, 1)) МБ по адресу $BaseUrl/$exeName"

if ($NoGitHub) { return }

# --- GitHub: тег ставится на коммит, из которого собрана эта версия ---
# git и gh пишут служебные сообщения в stderr; в PowerShell 5.1 при "Stop" это превращается
# в исключение даже при успехе, поэтому здесь проверяем только коды возврата.
$ErrorActionPreference = "Continue"
Push-Location $root
try {
  $versionFiles = @("src-tauri/tauri.conf.json", "src-tauri/Cargo.toml", "src-tauri/Cargo.lock", "package.json", "package-lock.json")
  $changed = git status --porcelain -- $versionFiles
  if ($changed) {
    git add -- $versionFiles
    git commit -q -m "Release v$Version"
    if ($LASTEXITCODE -ne 0) { throw "Не удалось закоммитить смену версии" }
  }
  git push -q
  if ($LASTEXITCODE -ne 0) { throw "Не удалось отправить коммит на GitHub" }
  $sha = (git rev-parse HEAD).Trim()

  $tag = "v$Version"
  $ghNotes = if ($Notes) { $Notes } else { "Videonarez $Version" }
  $notesTmp = Join-Path ([IO.Path]::GetTempPath()) "videonarez-release-$Version.md"
  [IO.File]::WriteAllText($notesTmp, "$ghNotes`n`nSHA-256 установщика: ``$((Get-FileHash $exe -Algorithm SHA256).Hash.ToLower())```n", $utf8)

  gh release view $tag 2>$null | Out-Null
  if ($LASTEXITCODE -eq 0) {
    Write-Host "Релиз $tag уже есть на GitHub — обновляю файлы"
    gh release upload $tag $exe $sig --clobber
  } else {
    gh release create $tag $exe $sig --target $sha --title "Videonarez $Version" --notes-file $notesTmp --latest
  }
  if ($LASTEXITCODE -ne 0) { throw "Не удалось создать или обновить релиз на GitHub" }
  Remove-Item $notesTmp -ErrorAction SilentlyContinue
  $url = (gh release view $tag --json url | ConvertFrom-Json).url
  Write-Host "GitHub: $url"
} finally {
  Pop-Location
}
