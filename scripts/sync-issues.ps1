$ErrorActionPreference = "Stop"

$repoRoot = Split-Path -Parent $PSScriptRoot
$backlogPath = Join-Path $repoRoot "docs\BACKLOG.json"
$backlog = Get-Content -Raw -Encoding UTF8 $backlogPath | ConvertFrom-Json

$allIssuesJson = & gh issue list --state all --limit 500 --json number,title,state,labels,body
$allIssues = $allIssuesJson | ConvertFrom-Json

$issueByNumber = @{}
$issueBySpawnId = @{}

foreach ($iss in $allIssues) {
  $issueByNumber[[int]$iss.number] = $iss
  if ($null -ne $iss.body -and $iss.body -match '<!-- backlog-id: ([a-z0-9\-]+) -->') {
    $issueBySpawnId[$Matches[1]] = $iss
  }
}

# 1. Unblock existing issues whose dependencies are all CLOSED
foreach ($rule in $backlog.unblock_rules) {
  $targetNum = [int]$rule.issue
  if (-not $issueByNumber.ContainsKey($targetNum)) { continue }
  $target = $issueByNumber[$targetNum]
  if ($target.state -ne "OPEN") { continue }

  $hasBlocked = $false
  foreach ($lbl in $target.labels) {
    if ($lbl.name -eq "blocked") { $hasBlocked = $true }
  }
  if (-not $hasBlocked) { continue }

  $allDepsClosed = $true
  foreach ($depNum in $rule.depends_on) {
    $d = [int]$depNum
    if (-not $issueByNumber.ContainsKey($d) -or $issueByNumber[$d].state -ne "CLOSED") {
      $allDepsClosed = $false
      break
    }
  }

  if ($allDepsClosed) {
    & gh issue edit $targetNum --remove-label "blocked" | Out-Null
    & gh issue comment $targetNum --body "前提 Issue が完了したため、`blocked` ラベルを自動解除しました（着手可能）。" | Out-Null
    Write-Host "Unblocked Issue #$targetNum"
  }
}

# 2. Spawn new issues whose dependencies are all CLOSED
foreach ($rule in $backlog.spawn_rules) {
  $id = $rule.id
  if ($issueBySpawnId.ContainsKey($id)) { continue }

  $ready = $true
  if ($null -ne $rule.depends_on_issues) {
    foreach ($depNum in $rule.depends_on_issues) {
      $d = [int]$depNum
      if (-not $issueByNumber.ContainsKey($d) -or $issueByNumber[$d].state -ne "CLOSED") {
        $ready = $false
        break
      }
    }
  }
  if ($ready -and $null -ne $rule.depends_on_ids) {
    foreach ($depId in $rule.depends_on_ids) {
      if (-not $issueBySpawnId.ContainsKey($depId) -or $issueBySpawnId[$depId].state -ne "CLOSED") {
        $ready = $false
        break
      }
    }
  }

  if ($ready) {
    $bodyText = $rule.body + "`n<!-- backlog-id: $id -->`n"
    $tmpFile = [System.IO.Path]::GetTempFileName()
    [System.IO.File]::WriteAllText($tmpFile, $bodyText, (New-Object System.Text.UTF8Encoding($false)))
    $labelArgs = @()
    foreach ($l in $rule.labels) {
      $labelArgs += "--label"
      $labelArgs += $l
    }
    $url = & gh issue create --title $rule.title --body-file $tmpFile @labelArgs
    Remove-Item -Force $tmpFile
    Write-Host "Spawned Issue ($id): $url"
  }
}
