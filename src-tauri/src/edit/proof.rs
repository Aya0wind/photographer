//! Soft proofing is a view operation. It never rewrites the edit recipe or pixels.
use crate::ipc::{run_blocking,SharedState};
use serde::Deserialize;
use std::{io::Read,sync::{Arc,Mutex,OnceLock},collections::VecDeque};
use tauri::State;
#[derive(Clone,Deserialize,serde::Serialize)]
#[serde(rename_all="camelCase")]
pub struct ProofOptions {profile:String,path:Option<String>,intent:String,bpc:bool,simulate_paper:bool,gamut:bool}
type Cached=(String,Arc<photocraft_cms::Transform>,Option<Arc<photocraft_cms::GamutCheck>>);
fn cache()->&'static Mutex<VecDeque<Cached>>{static VALUE:OnceLock<Mutex<VecDeque<Cached>>>=OnceLock::new();VALUE.get_or_init(Default::default)}
fn profile(options:&ProofOptions)->Result<photocraft_cms::Profile,String>{
    use photocraft_cms::Builtin;
    Ok(match options.profile.as_str(){
        "srgb"=>Builtin::Srgb.profile().clone(),"displayP3"=>Builtin::DisplayP3.profile().clone(),"adobeRgb"=>Builtin::AdobeRgbCompat.profile().clone(),"proPhotoRgb"=>Builtin::ProPhotoCompat.profile().clone(),
        "custom"=>{let path=options.path.as_ref().ok_or("请选择校样 ICC 文件")?;let mut bytes=Vec::new();std::fs::File::open(path).map_err(|e|e.to_string())?.take(4*1024*1024+1).read_to_end(&mut bytes).map_err(|e|e.to_string())?;if bytes.len()>4*1024*1024{return Err("校样 ICC 文件过大".into());}photocraft_cms::Profile::parse(&bytes).map_err(|e|e.to_string())?},
        _=>return Err("校样配置无效".into()),
    })
}
#[tauri::command]
pub async fn edit_preview_proof(state:State<'_,SharedState>,session_id:String,recipe:serde_json::Value,options:ProofOptions)->Result<tauri::ipc::Response,String>{
    let recipe=super::recipe::parse_recipe(&recipe)?;
    run_blocking(state.inner().clone(),move |_|{
        use image::ImageEncoder;
        use sha2::Digest;
        let base=super::preview::interactive_source(&session_id)?;
        let doc=super::photocraft::render_document(&base,&recipe)?;
        let source=photocraft_engine::color_cmds::document_profile(&doc);if source.channels()!=3{return Err("软打样目前需要 RGB 文档".into());}let proof=profile(&options)?;
        let intent=match options.intent.as_str(){"relative"=>photocraft_cms::Intent::RelativeColorimetric,"perceptual"=>photocraft_cms::Intent::Perceptual,"absolute"=>photocraft_cms::Intent::AbsoluteColorimetric,"saturation"=>photocraft_cms::Intent::Saturation,_=>return Err("校样意图无效".into())};
        let key=format!("{:x}:{:x}:{}",sha2::Sha256::digest(source.to_bytes().as_slice()),sha2::Sha256::digest(proof.to_bytes().as_slice()),serde_json::to_string(&options).map_err(|e|e.to_string())?);
        let cached=cache().lock().map_err(|_|"校样缓存不可用")?.iter().find(|(k,_,_)|k==&key).cloned();
        let (_,transform,gamut)=if let Some(value)=cached{value}else{
            let transform=Arc::new(photocraft_cms::Transform::proof(&source,&proof,photocraft_cms::Builtin::Srgb.profile(),intent,options.bpc,options.simulate_paper).map_err(|e|e.to_string())?);
            let gamut=if options.gamut{Some(Arc::new(photocraft_cms::GamutCheck::new(&source,&proof,photocraft_cms::gamut::DEFAULT_THRESHOLD).map_err(|e|e.to_string())?))}else{None};
            let value=(key,transform,gamut);let mut cache=cache().lock().map_err(|_|"校样缓存不可用")?;cache.push_front(value.clone());while cache.len()>4{cache.pop_back();}value
        };
        let pixels=photocraft_compose::flatten(&doc);let mut rgb:Vec<f32>=pixels.px.iter().flat_map(|p|p[..3].iter().copied()).collect();
        let warnings:Vec<bool>=pixels.px.iter().map(|p|gamut.as_ref().is_some_and(|g|p[3]>0.0&&g.out_of_gamut(&p[..3]))).collect();
        transform.apply(&mut rgb,3);
        let rgba:Vec<u8>=rgb.chunks_exact(3).enumerate().flat_map(|(i,c)|{let color=if warnings[i]{[1.0,0.0,1.0]}else{[c[0],c[1],c[2]]};[color[0],color[1],color[2],pixels.px[i][3]].map(|v|(v.clamp(0.0,1.0)*255.0).round() as u8)}).collect();
        let mut png=Vec::new();image::codecs::png::PngEncoder::new(&mut png).write_image(&rgba,doc.size.width,doc.size.height,image::ExtendedColorType::Rgba8).map_err(|e|e.to_string())?;
        Ok(tauri::ipc::Response::new(png))
    }).await
}
