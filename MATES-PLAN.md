# Mates plan (feature 6) — remove this file before the PR

Agreed with David 2026-11-23. First assembly: the pinch valve
(`Pinch Valve Assembly (Large).SLDASM`).

## Why a sidecar

A 3D PDF holds meshes and part names, never mates. So each CAD program gets a
small exporter that writes the mates as JSON, and that file rides inside the
PDF as an embedded file (`<assembly>.mates.json`).

## Order

1. SOLIDWORKS: `tools/cad-mates/solidworks/export-mates.ps1`. It attaches to
   the running SOLIDWORKS (COM), reads the active assembly's top-level mates and
   writes `<assembly>.mates.json` next to the assembly. Run it with
   `export-mates.cmd` while the assembly is open.
2. pdfcraft: "Attach mates…" in the Model Tree puts the JSON into the PDF; the
   viewer reads it back from the embedded file.
3. Viewer, first version:
   - a Mates folder under each part in the Model Tree (like SOLIDWORKS);
     clicking a mate highlights its axis or plane;
   - drag a part and it moves only the way its mates leave free: turning about
     one axis (concentric + coincident, hinge) or sliding along one line
     (concentric alone, slider, slot), stopped at limit mates;
   - parts the mates fully fix, and fixed components, stay put.
4. Later: chains (linkages, gears, rack and pinion, cams), which need a real
   constraint solver; mates inside flexible sub-assemblies.
5. Fusion 360 (Python add-in script, joints) and FreeCAD (macro, Assembly
   workbench joints) write the same JSON. Do not forget these two.

## JSON (version 1)

Lengths in mm, assembly coordinates, the same frame the PRC uses.

```json
{
  "format": "pdfcraft-mates", "version": 1,
  "source": "SOLIDWORKS 2024", "assembly": "Pinch Valve Assembly (Large)",
  "components": [
    { "name": "Pinch Valve Pusher-1", "file": "Pinch Valve Pusher.SLDPRT",
      "fixed": false, "suppressed": false,
      "transform": [r11,r12,r13,r21,r22,r23,r31,r32,r33,tx,ty,tz,scale] }
  ],
  "mates": [
    { "name": "Concentric3", "type": "MateConcentric", "type_id": 1,
      "alignment": "aligned", "suppressed": false,
      "value": null, "min": null, "max": null,
      "entities": [
        { "component": "Pinch Valve Pusher-1", "kind": "cylinder",
          "point": [x,y,z], "direction": [x,y,z], "radius": 6.0, "radius2": 0 }
      ] }
  ]
}
```

`type` is SOLIDWORKS' own feature type name (MateCoincident, MateConcentric,
MateDistanceDim, MateLock, MateSlot, MateHinge, …); `kind` is point, line,
circle, plane, cylinder, sphere, cone or other. Values are mm or degrees.

Matching to the PDF: component names (`Pusher-1`, `Sub-1/Part-1`) are compared
with PRC part names; a part the viewer cannot match is listed, not guessed.
