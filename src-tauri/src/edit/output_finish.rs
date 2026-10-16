//! Output-only finishing, after size and edits; never changes the stored recipe.
use photocraft_engine::{doc::Document, Session};
use serde::{Deserialize, Serialize};
use serde_json::json;
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Options {
    pub bit_depth: u8,
    pub tiff_compression: Compression,
    pub sharpen: f64,
    pub watermark: Option<Watermark>,
}
impl Default for Options {
    fn default() -> Self {
        Self {
            bit_depth: 16,
            tiff_compression: Compression::Deflate,
            sharpen: 0.0,
            watermark: None,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum Compression {
    None,
    Lzw,
    #[default]
    Deflate,
    PackBits,
}
impl Compression {
    pub(super) fn native(self) -> photocraft_codecs::TiffCompression {
        use photocraft_codecs::TiffCompression as C;
        match self {
            Self::None => C::None,
            Self::Lzw => C::Lzw,
            Self::Deflate => C::Deflate,
            Self::PackBits => C::PackBits,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Watermark {
    pub text: String,
    pub opacity: f64,
    pub size_rel: f64,
    pub position: String,
}
pub(super) fn validate(value: &Options) -> Result<(), String> {
    if ![8, 16].contains(&value.bit_depth)
        || !value.sharpen.is_finite()
        || !(0.0..=200.0).contains(&value.sharpen)
    {
        return Err("输出位深或锐化参数无效".into());
    }
    if let Some(w) = &value.watermark {
        if w.text.chars().count() > 300
            || !w.opacity.is_finite()
            || !(0.0..=1.0).contains(&w.opacity)
            || !w.size_rel.is_finite()
            || !(0.005..=0.1).contains(&w.size_rel)
            || !["topLeft", "topRight", "bottomLeft", "bottomRight"].contains(&w.position.as_str())
        {
            return Err("水印参数无效".into());
        }
    }
    Ok(())
}
pub(super) fn apply(doc: Document, options: &Options) -> Result<Document, String> {
    if options.sharpen == 0.0
        && options
            .watermark
            .as_ref()
            .is_none_or(|w| w.text.trim().is_empty())
    {
        return Ok(doc);
    }
    let mut session = Session::new();
    session.add_document(super::photocraft::processing_input(&doc)?, None);
    if options.sharpen > 0.0 {
        super::photocraft::execute(
            &mut session,
            "filter.sharpen.unsharpMask",
            json!({"amount":options.sharpen,"radius":0.6,"threshold":2}),
        )?;
    }
    if let Some(w) = &options.watermark {
        if !w.text.trim().is_empty() {
            let width = f64::from(doc.size.width);
            let height = f64::from(doc.size.height);
            let size = (width * w.size_rel).clamp(1.0, 512.0);
            let right = w.position.ends_with("Right");
            let bottom = w.position.starts_with("bottom");
            super::photocraft::execute(
                &mut session,
                "type.create",
                json!({"text":w.text,"color":"#FFFFFF","box":[width*0.03,if bottom{(height-size*1.8).max(0.0)}else{height*0.03},width*0.94,size*1.5],"size":size*72.0/f64::from(doc.resolution_dpi),"align":if right{"right"}else{"left"},"name":"Watermark"}),
            )?;
            super::photocraft::execute(
                &mut session,
                "layer.setProps",
                json!({"opacity":w.opacity}),
            )?;
        }
    }
    super::photocraft::active_document(&session)
}
