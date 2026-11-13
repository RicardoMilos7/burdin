# Validates the structure of the conformance tests (directives and error codes).
# It does not compile anything. Run from any directory: pwsh tests/check-tests.ps1
param([string]$Root = $PSScriptRoot)

$readme = Get-Content -Raw (Join-Path $Root 'README.md')
$known = [regex]::Matches($readme, '\| (E\d{4}) \|') | ForEach-Object { $_.Groups[1].Value } | Sort-Object -Unique
$kinds = 'run', 'run-abort', 'compile-fail', 'burdin-test', 'undecided'

$problems = New-Object System.Collections.Generic.List[string]
$counts = @{}
$total = 0

$files = Get-ChildItem -Path $Root -Recurse -Filter *.bd | Where-Object {
    (Get-Content -Raw $_.FullName) -match '(?m)^//@ '
}

foreach ($f in $files) {
    $total++
    $rel = $f.FullName.Substring($Root.Length + 1)
    $lines = Get-Content $f.FullName
    $directives = @($lines | Where-Object { $_ -match '^//@ ' } | ForEach-Object { $_.Substring(4).Trim() })
    $expected = @($lines | Where-Object { $_ -match '^//> ' })
    $markers = @()
    for ($i = 0; $i -lt $lines.Count; $i++) {
        foreach ($m in [regex]::Matches($lines[$i], '//~ (E\d{4})')) {
            $markers += [pscustomobject]@{ Line = $i + 1; Code = $m.Groups[1].Value }
        }
    }

    $kind = @($directives | Where-Object {
        $d = $_
        $kinds | Where-Object { $d -eq $_ -or $d.StartsWith($_ + ' ') -or $d.StartsWith($_ + ':') }
    })
    $kindNames = @($kind | ForEach-Object { ($_ -split '[ :]')[0] })
    if ($kindNames.Count -eq 0) { $problems.Add("${rel}: no test kind directive"); continue }
    $primary = if ($kindNames -contains 'undecided') { 'undecided' } else { $kindNames | Select-Object -First 1 }
    if ($counts.ContainsKey($primary)) { $counts[$primary]++ } else { $counts[$primary] = 1 }

    if ($primary -ne 'undecided' -and $kindNames.Count -gt 1) { $problems.Add("${rel}: more than one test kind") }
    if ($primary -eq 'undecided') { continue }

    switch ($primary) {
        'run' { if ($markers.Count -gt 0) { $problems.Add("${rel}: run test has //~ markers") } }
        'run-abort' {
            if (-not ($directives -match '^stderr:')) { $problems.Add("${rel}: run-abort needs a //@ stderr: line") }
        }
        'compile-fail' {
            if ($markers.Count -eq 0) { $problems.Add("${rel}: compile-fail needs at least one //~ marker") }
            if ($expected.Count -gt 0) { $problems.Add("${rel}: compile-fail must not have //> lines") }
        }
        'burdin-test' {
            if (-not ($directives -match '^burdin-test passed=\d+ failed=\d+$')) { $problems.Add("${rel}: burdin-test needs passed=N failed=M") }
        }
    }
    foreach ($m in $markers) {
        if ($known -notcontains $m.Code) { $problems.Add("${rel}:$($m.Line): unknown error code $($m.Code)") }
    }
    if ($primary -ne 'compile-fail' -and $primary -ne 'undecided' -and $markers.Count -gt 0) { }
}

"Checked $total tests:"
$counts.GetEnumerator() | Sort-Object Name | ForEach-Object { "  {0,-13} {1}" -f $_.Name, $_.Value }

$used = $files | ForEach-Object { [regex]::Matches((Get-Content -Raw $_.FullName), '//~ (E\d{4})') | ForEach-Object { $_.Groups[1].Value } } | Sort-Object -Unique
$unused = $known | Where-Object { $used -notcontains $_ }
if ($unused) { "Error codes in the registry with no test: $($unused -join ', ')" }

if ($problems.Count -gt 0) {
    ""
    "Problems:"
    $problems | ForEach-Object { "  $_" }
    exit 1
}
"No structural problems."
