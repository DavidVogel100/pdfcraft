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

# SOLIDWORKS 2016 and later list themselves as "SolidWorks_PID_<n>" in the
# running object table, not under SldWorks.Application.
Add-Type -TypeDefinition @'
using System; using System.Runtime.InteropServices; using System.Runtime.InteropServices.ComTypes;
public static class SwRot {
  [DllImport("ole32.dll")] static extern int GetRunningObjectTable(int r, out IRunningObjectTable t);
  [DllImport("ole32.dll")] static extern int CreateBindCtx(int r, out IBindCtx c);
  public static object Find() {
    IRunningObjectTable t; GetRunningObjectTable(0, out t);
    IEnumMoniker e; t.EnumRunning(out e);
    IBindCtx c; CreateBindCtx(0, out c);
    var m = new IMoniker[1];
    while (e.Next(1, m, IntPtr.Zero) == 0) {
      string n; m[0].GetDisplayName(c, null, out n);
      if (n.StartsWith("SolidWorks_PID_", StringComparison.OrdinalIgnoreCase)) { object o; t.GetObject(m[0], out o); return o; }
    }
    return null;
  }
}
'@
$sw = $null
try { $sw = [SwRot]::Find() } catch { }
if ($null -eq $sw) {
    try { $sw = [System.Runtime.InteropServices.Marshal]::GetActiveObject('SldWorks.Application') } catch { }
}
if ($null -eq $sw) {
    Stop-WithMessage "SOLIDWORKS is not open (or it runs as administrator and this script does not). Open the assembly in SOLIDWORKS, then run this again."
}

# Calls through IDispatch by name. PowerShell's own COM adapter needs the
# SOLIDWORKS type library and fails with TYPE_E_ELEMENTNOTFOUND without it.
function X($obj, [string]$name) {
    if ($null -eq $obj) { return $null }
    $flags = [System.Reflection.BindingFlags]'InvokeMethod, GetProperty'
    [System.__ComObject].InvokeMember($name, $flags, $null, $obj, [object[]]$args)
}
function Try-X($obj, [string]$name) {
    try { X $obj $name @args } catch { $null }
}

$doc = X $sw 'ActiveDoc'
if ($null -eq $doc) { Stop-WithMessage "No document is open in SOLIDWORKS." }
$path = X $doc 'GetPathName'
if ($path -notmatch '\.sldasm$') { Stop-WithMessage "The active window is not an assembly: $(X $doc 'GetTitle')" }

function To-Mm($v) { [math]::Round([double]$v * 1000.0, 6) }
function Round-Unit($v) { [math]::Round([double]$v, 9) }

$kinds = @{ 0 = 'point'; 1 = 'line'; 2 = 'plane'; 3 = 'cylinder'; 4 = 'sphere'; 5 = 'set'; 6 = 'cone'; 7 = 'swept'; 8 = 'multiple'; 9 = 'surface'; 10 = 'ellipse' }
$aligns = @{ 0 = 'aligned'; 1 = 'anti-aligned'; 2 = 'closest' }

Write-Host "Reading $([IO.Path]::GetFileName($path)) ..."

# Components, every level, with where each one sits in the assembly.
$components = New-Object System.Collections.ArrayList
foreach ($c in @(X $doc 'GetComponents' $false)) {
    if ($null -eq $c) { continue }
    $xf = $null
    try {
        $a = X (X $c 'Transform2') 'ArrayData'
        $xf = @(
            (Round-Unit $a[0]), (Round-Unit $a[1]), (Round-Unit $a[2]),
            (Round-Unit $a[3]), (Round-Unit $a[4]), (Round-Unit $a[5]),
            (Round-Unit $a[6]), (Round-Unit $a[7]), (Round-Unit $a[8]),
            (To-Mm $a[9]), (To-Mm $a[10]), (To-Mm $a[11]), (Round-Unit $a[12]))
    } catch { }
    $file = $null
    try { $file = [IO.Path]::GetFileName((X $c 'GetPathName')) } catch { }
    [void]$components.Add([ordered]@{
        name       = (X $c 'Name2')
        file       = $file
        fixed      = [bool](Try-X $c 'IsFixed')
        suppressed = [bool](Try-X $c 'IsSuppressed')
        transform  = $xf
    })
}

# One mate feature to a record, or $null when the feature is not a mate.
function Read-Mate($feat) {
    $m = Try-X $feat 'GetSpecificFeature2'
    if ($null -eq $m) { return $null }
    $typeId = Try-X $m 'Type'
    if ($null -eq $typeId) { return $null }
    $count = Try-X $m 'GetMateEntityCount'
    if ($null -eq $count) { return $null }
    $typeName = X $feat 'GetTypeName2'
    $isAngle = $typeName -match 'Angle'

    $entities = @()
    for ($i = 0; $i -lt [int]$count; $i++) {
        $e = Try-X $m 'MateEntity' $i
        if ($null -eq $e) { continue }
        $owner = $null
        $rc = Try-X $e 'ReferenceComponent'
        if ($null -ne $rc) { $owner = Try-X $rc 'Name2' }
        $kindId = Try-X $e 'ReferenceType2'
        if ($null -eq $kindId) { $kindId = -1 }
        $kind = if ($kinds.ContainsKey([int]$kindId)) { $kinds[[int]$kindId] } else { 'other' }
        $p = Try-X $e 'EntityParams'
        $rec = [ordered]@{ component = $owner; kind = $kind; kind_id = [int]$kindId }
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
        $dd = Try-X $m 'DisplayDimension'
        if ($null -ne $dd) { $value = [math]::Round([double](X (X $dd 'GetDimension2' 0) 'SystemValue') * $scale, 6) }
    } catch { }
    try {
        $hi = [double](X $m 'MaximumVariation')
        $lo = [double](X $m 'MinimumVariation')
        if ($hi -ne $lo) { $max = [math]::Round($hi * $scale, 6); $min = [math]::Round($lo * $scale, 6) }
    } catch { }

    $align = $null
    $a = Try-X $m 'Alignment'
    if ($null -ne $a) { $align = if ($aligns.ContainsKey([int]$a)) { $aligns[[int]$a] } else { "$a" } }

    [ordered]@{
        name       = (X $feat 'Name')
        type       = $typeName
        type_id    = [int]$typeId
        alignment  = $align
        suppressed = [bool](Try-X $feat 'IsSuppressed')
        value      = $value
        min        = $min
        max        = $max
        entities   = $entities
    }
}

# The Mates folder, and any folders the user made inside it.
$mates = New-Object System.Collections.ArrayList
function Walk-Mates($parent) {
    $f = X $parent 'GetFirstSubFeature'
    while ($null -ne $f) {
        $rec = $null
        try { $rec = Read-Mate $f } catch { }
        if ($null -ne $rec) { [void]$mates.Add($rec) }
        elseif ((X $f 'GetTypeName2') -eq 'FtrFolder') { Walk-Mates $f }
        $f = X $f 'GetNextSubFeature'
    }
}
$feat = X $doc 'FirstFeature'
while ($null -ne $feat) {
    if ((X $feat 'GetTypeName2') -eq 'MateGroup') { Walk-Mates $feat }
    $feat = X $feat 'GetNextFeature'
}

$result = [ordered]@{
    format     = 'pdfcraft-mates'
    version    = 1
    source     = "SOLIDWORKS $(Try-X $sw 'RevisionNumber')"
    assembly   = [IO.Path]::GetFileNameWithoutExtension($path)
    units      = 'mm, degrees'
    components = @($components)
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
