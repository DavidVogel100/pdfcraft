# 3D dimensions and notes: agreed design (2026-11-18)

Agreed with David on 2026-11-18. Builds on feat/3d-measure. This file is for planning only: remove it before the PR goes upstream.

## Workflow
- Pick the part: click it in the picture (the Model Tree highlights it in blue), or click it in the tree.
- Tick **Dimension** (beside Measure). If a part is already selected, it is isolated at once and its Dimension layer becomes active. If nothing is selected, the next part clicked is used.
- The first time a part has no Dimension layer, a popup asks: "Create a Dimension layer for <part>? It will show on all <n> copies." with **Yes** and **Cancel**. After that there is no popup; measurements go straight onto that layer.
- Each new reading shows **Keep** and **✕**. Keep draws it as a proper dimension: extension lines, arrows and text. ✕ drops it, so stray clicks leave nothing behind.
- Kept dimensions can be moved by dragging their text, and deleted.
- Tolerances (±) and the number of decimal places can be set per dimension.

## Model Tree
- A part that has layers gets a **>** and a ruler icon. Under it are **Dimensions** and **Notes**, each with an eye to show or hide.
- Layers show only while their part is isolated, so the whole assembly never becomes a mess.
- Sub-assemblies (e.g. Def Electronic Box Assembly) can have their own layers, for dimensions between their parts.

## Shared parts
- Layers belong to the **part**, not one copy: every copy of, say, Threaded Insert Brass 10-32 shows the same dimensions.
- How it is shown:
  - In the tree, the ruler icon and "Dimensions (shared by 9 copies)" appear under every copy.
  - While the part is isolated, a thin bar over the view reads "Dimensions: <part> · shared by <n> copies".
  - The popup names the part and the number of copies.
  - The part name goes on the print.

## Views
- A Dimension layer stores its camera (Front, Top, Right or the angle set) as a saved view. Clicking the layer turns the part to that view.
- A part may have several, e.g. "Front, dims" and "Top, dims", like SOLIDWORKS drawing views.

## Notes
- A leader note is pinned to a point on the part and moves when the part is turned.
- A sheet note is fixed on the paper, e.g. "Break all edges" or a title block.

## Saving in the PDF
- Dimensions are written as standard PDF 3D measurements (3DMeasure: linear, radial, angular) in the layer's saved 3D view, so Adobe Acrobat shows them too.
- What the standard cannot hold (the part a layer belongs to, layer names, note style, tolerances) goes in a pdfcraft-private dictionary in the same file. Other programs ignore it.
- Saving is an incremental update: the original bytes are untouched, signatures stay valid, and it is easy to undo.

## Printing
- Prints the isolated part in the layer's view with its dimensions and notes, using square-on (orthographic) views so the dimensions match the paper.
- The model prints as a picture; lines and text print as vectors, sharp on the DesignJet. Never send anything to the DesignJet without David's OK for that print.
- There is an optional small title block with the part name.

## Not doing
- Attaching .slddrw drawing PDFs as extra pages: a large assembly would need far too many drawings.
- Reading SOLIDWORKS MBD/DimXpert annotations: David doesn't use them.

## Order (one upstream PR each)
1. Dimension layers: create, keep, move, delete and tolerances, saved in the PDF.
2. Notes layers.
3. Printing a part with its layers.
