//! The 3D models on a page (ISO 32000-2 §13.6), read so a renderer can draw them.

pub use pdfcraft_annot::{MAX_MODEL_BYTES, Model3d, Model3dKind};

use crate::Document;

impl Document {
    /// The `/Annots` positions of the 3D annotations on 0-based `page`, in order.
    pub fn model_3d_annotations(&self, page: usize) -> Result<Vec<usize>, String> {
        let editor = self.editor.as_ref().ok_or("the document can't be read")?;
        pdfcraft_annot::model_3d_annotations(&editor.cos, page).map_err(|e| e.to_string())
    }

    /// The dimension and note layers kept on the 3D annotation at `/Annots` position `index` on
    /// 0-based `page`.
    pub fn model_3d_layers(&self, page: usize, index: usize) -> Result<Vec<pdfcraft_annot::Layer3d>, String> {
        let editor = self.editor.as_ref().ok_or("the document can't be read")?;
        pdfcraft_annot::model_3d_layers(&editor.cos, page, index).map_err(|e| e.to_string())
    }

    /// The mates attached to the document (a `*.mates.json` attachment, as the CAD exporters in
    /// `tools/cad-mates` write them): the attachment's name and its text.
    pub fn model_3d_mates(&self) -> Option<(String, String)> {
        let editor = self.editor.as_ref()?;
        let wanted = |key: &str| key.to_ascii_lowercase().ends_with(".mates.json");
        let (key, bytes) = pdfcraft_organize::attachment_named(&editor.cos, wanted, 64 << 20)?;
        Some((key, String::from_utf8(bytes).ok()?))
    }

    /// The decoded model of the 3D annotation at `/Annots` position `index` on 0-based `page`.
    pub fn model_3d(&self, page: usize, index: usize) -> Result<Model3d, String> {
        let editor = self.editor.as_ref().ok_or("the document can't be read")?;
        pdfcraft_annot::model_3d(&editor.cos, page, index).map_err(|e| e.to_string())
    }
}
