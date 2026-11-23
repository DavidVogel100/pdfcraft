# Writes the mates of the assembly open in SOLIDWORKS to <assembly>.mates.json,
# next to the assembly, for pdfcraft's 3D viewer (see MATES-PLAN.md).
# Run export-mates.cmd while the assembly is the active window in SOLIDWORKS.
# It only reads: nothing in the assembly is changed or saved.
param([string]$Out)

$ErrorActionPreference = 'Stop'

function Stop-WithMessage([string]$text) {
    Write-Host ""
    Write-Host $text -ForegroundColor Red
    exit 1
}

try {
    $sw = [System.Runtime.InteropServices.Marshal]::GetActiveObject('SldWorks.Application')
} catch {
    Stop-WithMessage "SOLIDWORKS is not open (or it runs as administrator and this script does not). Open the assembly in SOLIDWORKS, then run this again."
}

$doc = $sw.ActiveDoc
if ($null -eq $doc) { Stop-WithMessage "No document is open in SOLIDWORKS." }
$path = $doc.GetPathName()
if ($path -notmatch '\.sldasm$') { Stop-WithMessage "The active window is not an assembly: $($doc.GetTitle())" }

function To-Mm($v) { [math]::Round([double]$v * 1000.0, 6) }
function Round-Unit($v) { [math]::Round([double]$v, 9) }

$kinds = @{ 0 = 'point'; 1 = 'line'; 2 = 'circle'; 3 = 'plane'; 4 = 'cylinder'; 5 = 'sphere'; 6 = 'set'; 7 = 'cone' }
$aligns = @{ 0 = 'aligned'; 1 = 'anti-aligned'; 2 = 'closest' }

# Components, every level, with where each one sits in the assembly.
$components = @()
foreach ($c in @($doc.GetComponents($false))) {
    if ($null -eq $c) { continue }
    $xf = $null
    try {
        $a = $c.Transform2.ArrayData
        $xf = @(
            (Round-Unit $a[0]), (Round-Unit $a[1]), (Round-Unit $a[2]),
            (Round-Unit $a[3]), (Round-Unit $a[4]), (Round-Unit $a[5]),
            (Round-Unit $a[6]), (Round-Unit $a[7]), (Round-Unit $a[8]),
            (To-Mm $a[9]), (To-Mm $a[10]), (To-Mm $a[11]), (Round-Unit $a[12]))
    } catch { }
    $file = $null
    try { $file = [IO.Path]::GetFileName($c.GetPathName()) } catch { }
    $components += [ordered]@{
        name       = $c.Name2
        file       = $file
        fixed      = [bool]$c.IsFixed()
        suppressed = [bool]$c.IsSuppressed()
        transform  = $xf
    }
}

# One mate feature to a record, or $null when the feature is not a mate.
function Read-Mate($feat) {
    $m = $feat.GetSpecificFeature2()
    if ($null -eq $m) { return $null }
    try { $typeId = [int]$m.Type } catch { return $null }
    $typeName = $feat.GetTypeName2()
    $isAngle = $typeName -match 'Angle'

    $entities = @()
    $count = 0
    try { $count = [int]$m.MateEntityCount } catch { }
    for ($i = 0; $i -lt $count; $i++) {
        $e = $m.MateEntity($i)
        if ($null -eq $e) { continue }
        $owner = $null
        try { if ($null -ne $e.ReferenceComponent) { $owner = $e.ReferenceComponent.Name2 } } catch { }
        $kindId = -1
        try { $kindId = [int]$e.ReferenceType2 } catch { }
        $kind = if ($kinds.ContainsKey($kindId)) { $kinds[$kindId] } else { 'other' }
        $p = $null
        try { $p = $e.EntityParams } catch { }
        $rec = [ordered]@{ component = $owner; kind = $kind; kind_id = $kindId }
        if ($null -ne $p -and $p.Count -ge 6) {
            $rec.point = @((To-Mm $p[0]), (To-Mm $p[1]), (To-Mm $p[2]))
            $rec.direction = @((Round-Unit $p[3]), (Round-Unit $p[4]), (Round-Unit $p[5]))
            if ($p.Count -ge 8) { $rec.radius = (To-Mm $p[6]); $rec.radius2 = (To-Mm $p[7]) }
        }
        $entities += $rec
    }

    # Distance or angle, and the limits of a limit mate, in mm or degrees.
    $scale = if ($isAngle) { 180.0 / [math]::PI } else { 1000.0 }
    $value = $null; $min = $null; $max = $null
    try {
        $d = $m.DisplayDimension
        if ($null -ne $d) { $value = [math]::Round($d.GetDimension2(0).SystemValue * $scale, 6) }
    } catch { }
    try {
        $hi = [double]$m.MaximumVariation
        $lo = [double]$m.MinimumVariation
        if ($hi -ne $lo) { $max = [math]::Round($hi * $scale, 6); $min = [math]::Round($lo * $scale, 6) }
    } catch { }

    $align = $null
    try { $a = [int]$m.Alignment; $align = if ($aligns.ContainsKey($a)) { $aligns[$a] } else { "$a" } } catch { }

    [ordered]@{
        name       = $feat.Name
        type       = $typeName
        type_id    = $typeId
        alignment  = $align
        suppressed = [bool]$feat.IsSuppressed()
        value      = $value
        min        = $min
        max        = $max
        entities   = $entities
    }
}

# The Mates folder, and any folders the user made inside it.
$mates = New-Object System.Collections.ArrayList
function Walk-Mates($parent) {
    $f = $parent.GetFirstSubFeature()
    while ($null -ne $f) {
        $rec = $null
        try { $rec = Read-Mate $f } catch { }
        if ($null -ne $rec) { [void]$mates.Add($rec) }
        elseif ($f.GetTypeName2() -eq 'FtrFolder') { Walk-Mates $f }
        $f = $f.GetNextSubFeature()
    }
}
$feat = $doc.FirstFeature()
while ($null -ne $feat) {
    if ($feat.GetTypeName2() -eq 'MateGroup') { Walk-Mates $feat }
    $feat = $feat.GetNextFeature()
}

$result = [ordered]@{
    format     = 'pdfcraft-mates'
    version    = 1
    source     = "SOLIDWORKS $($sw.RevisionNumber())"
    assembly   = [IO.Path]::GetFileNameWithoutExtension($path)
    units      = 'mm, degrees'
    components = $components
    mates      = @($mates)
}

if (-not $Out) { $Out = [IO.Path]::ChangeExtension($path, '.mates.json') }
$json = $result | ConvertTo-Json -Depth 10
[IO.File]::WriteAllText($Out, $json, (New-Object System.Text.UTF8Encoding($false)))

Write-Host ""
Write-Host "$($components.Count) components and $($mates.Count) mates written to:" -ForegroundColor Green
Write-Host "  $Out"
Write-Host ""
$mates | Group-Object { $_.type } | Sort-Object Count -Descending |
    ForEach-Object { Write-Host ("  {0,4}  {1}" -f $_.Count, $_.Name) }
