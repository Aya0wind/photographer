//! Bypass groups in a disposable processing recipe; never erase saved parameters.
use super::recipe::{EditRecipe,Development};
pub(super) const NAMES:[&str;14]=["light","color","curves","parametricCurve","levels","hsl","colorBalance","blackWhite","selectiveColor","colorGrading","detail","effects","pointColors","primaries"];
pub(super) fn effective(recipe:&EditRecipe)->Option<EditRecipe>{
    let disabled=&recipe.advanced.as_ref()?.disabled_groups;
    if disabled.is_empty(){return None;}
    let mut value=recipe.clone();
    let a=value.advanced.as_mut()?;
    a.disabled_groups.clear();
    let defaults=Development::default();
    for group in disabled {
        match group.as_str(){
            "light"=>{a.exposure=0.0;if let Some(b)=value.adjustments.as_mut(){b.brightness=0.0;b.contrast=0.0;}if let Some(d)=a.development.as_mut(){d.highlights=0.0;d.shadows=0.0;d.whites=0.0;d.blacks=0.0;}},
            "color"=>{a.temperature=0.0;a.tint=0.0;a.vibrance=0.0;if let Some(b)=value.adjustments.as_mut(){b.saturation=0.0;}},
            "curves"=>{a.curves.clear();a.channel_curves=Default::default();},
            "parametricCurve"=>a.parametric_curve=None,
            "pointColors"=>a.point_colors.clear(),"primaries"=>a.primaries=None,
            "levels"=>a.levels=None,"hsl"=>a.hsl_eight=None,
            "colorBalance"=>a.color_balance=None,"blackWhite"=>a.black_white=None,
            "selectiveColor"=>a.selective_color=None,"colorGrading"=>a.color_grading=None,
            "detail"=>if let Some(d)=a.development.as_mut(){d.sharpen_amount=0.0;d.sharpen_radius=defaults.sharpen_radius;d.sharpen_detail=defaults.sharpen_detail;d.sharpen_masking=0.0;d.noise_luminance=0.0;d.noise_color=0.0;},
            "effects"=>if let Some(d)=a.development.as_mut(){d.texture=0.0;d.clarity=0.0;d.dehaze=0.0;d.grain_amount=0.0;d.vignette_amount=0.0;},
            _=>{},
        }
    }
    Some(value)
}
