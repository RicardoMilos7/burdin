# Validates the structure of the conformance tests (directives and error codes).
# It does not compile anything. Run from any directory: pwsh tests/check-tests.ps1
param([string]$Root = $PSScriptRoot)

$readme = Get-Content -Raw (Join-Path $Root 'README.md')
$exceptionsHeading = '### Codes without a compile-fail test'
# The registry table stops at the documented-exception subsection; a row in that subsection is not a registry
# entry.
$registry = if ($readme -match "(?s)(## Error codes.*?)$([regex]::Escape($exceptionsHeading))") {
    $Matches[1]
} else { $readme }
$known = [regex]::Matches($registry, '\| (E\d{4}) \|') | ForEach-Object { $_.Groups[1].Value } | Sort-Object -Unique
$exceptions = if ($readme -match "(?s)$([regex]::Escape($exceptionsHeading))(.*?)(\r?\n## |\z)") {
    [regex]::Matches($Matches[1], '(?m)^\| (E\d{4}) \|([^|]*)\|') |
        ForEach-Object { [pscustomobject]@{ Code = $_.Groups[1].Value; Reason = $_.Groups[2].Value.Trim() } }
} else { @() }
$kinds = 'run', 'run-abort', 'compile-fail', 'test-compile-fail', 'burdin-test', 'undecided'

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
        'test-compile-fail' {
            if ($markers.Count -eq 0) { $problems.Add("${rel}: test-compile-fail needs at least one //~ marker") }
            if ($expected.Count -gt 0) { $problems.Add("${rel}: test-compile-fail must not have //> lines") }
        }
        'burdin-test' {
            if (-not ($directives -match '^burdin-test passed=\d+ failed=\d+( ignored=\d+)?$')) { $problems.Add("${rel}: burdin-test needs passed=N failed=M [ignored=N]") }
        }
    }
    foreach ($m in $markers) {
        if ($known -notcontains $m.Code) { $problems.Add("${rel}:$($m.Line): unknown error code $($m.Code)") }
    }
}

"Checked $total tests:"
$counts.GetEnumerator() | Sort-Object Name | ForEach-Object { "  {0,-13} {1}" -f $_.Name, $_.Value }

$used = $files | ForEach-Object { [regex]::Matches((Get-Content -Raw $_.FullName), '//~ (E\d{4})') | ForEach-Object { $_.Groups[1].Value } } | Sort-Object -Unique

# Every registered code needs a compile-fail marker (or a test-compile-fail one) or a documented reason
# (issue #445).
if ($readme -notmatch [regex]::Escape($exceptionsHeading)) {
    $problems.Add("README.md: missing the `$exceptionsHeading` section")
}
foreach ($e in $exceptions) {
    if ($known -notcontains $e.Code) {
        $problems.Add("README.md: documented exception $($e.Code) is not in the registry")
    } elseif ($used -contains $e.Code) {
        $problems.Add("README.md: $($e.Code) has a compile-fail test; remove it from the documented exceptions")
    }
    if ([string]::IsNullOrWhiteSpace($e.Reason)) {
        $problems.Add("README.md: the exception for $($e.Code) has no reason")
    }
}
$unused = $known | Where-Object { $used -notcontains $_ }
foreach ($code in $unused) {
    if (-not ($exceptions | Where-Object { $_.Code -eq $code })) {
        $problems.Add("error code $code has no compile-fail test; add one or document why it cannot have one")
    }
}

if ($problems.Count -gt 0) {
    ""
    "Problems:"
    $problems | ForEach-Object { "  $_" }
    exit 1
}
"No structural problems."
