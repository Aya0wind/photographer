//! Read-only full-size inspection in the same killable boundary as export.
//! Prepare once, then compose only requested rectangles. Closing frees the source.
use std::{collections::HashMap,path::Path,sync::{Arc,Mutex,OnceLock,atomic::{AtomicBool,AtomicU64,Ordering}},time::Duration};
use serde::{Serialize,Deserialize};
use crate::ipc::{run_blocking,SharedState};
use tauri::State;

#[derive(Clone,Serialize)]
#[serde(rename_all="camelCase")]
pub struct DetailStatus {id:String,phase:String,width:u32,height:u32,revision:u64,busy:bool,error:Option<String>,cancelled:bool}
#[derive(Clone,Serialize,Deserialize)]
pub(super) struct View {pub revision:u64,pub x:f64,pub y:f64,pub edge:u32,pub edges:bool}
pub(super) struct Control {
    pub cancel:AtomicBool,
    pub status:Mutex<DetailStatus>,
    pub view:Mutex<Option<View>>,
    image:Mutex<Option<(u64,Vec<u8>)>>,
    next:AtomicU64,
    touched:AtomicU64,
}
fn now()->u64{std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs()}
fn jobs()->&'static Mutex<HashMap<String,Arc<Control>>>{static VALUE:OnceLock<Mutex<HashMap<String,Arc<Control>>>>=OnceLock::new();VALUE.get_or_init(Default::default)}
fn control(id:&str)->Result<Arc<Control>,String>{
    let job=jobs().lock().map_err(|_|"细节作业锁不可用")?.get(id).cloned().ok_or("细节作业已关闭")?;
    job.touched.store(now(),Ordering::Release);Ok(job)
}
impl Control {
    pub(super) fn stopped(&self)->bool{self.cancel.load(Ordering::Acquire)||now().saturating_sub(self.touched.load(Ordering::Acquire))>30}
    pub(super) fn phase(&self,phase:&str){if let Ok(mut status)=self.status.lock(){status.phase=phase.into();}}
    pub(super) fn ready(&self,width:u32,height:u32){if let Ok(mut status)=self.status.lock(){status.width=width;status.height=height;status.phase="ready".into();status.busy=self.view.lock().map(|v|v.is_some()).unwrap_or(false);}}
    pub(super) fn image(&self,revision:u64,bytes:Vec<u8>){
        let latest=self.next.load(Ordering::Acquire);
        if revision!=latest{return;}
        if let Ok(mut image)=self.image.lock(){*image=Some((revision,bytes));}
        if let Ok(mut status)=self.status.lock(){status.revision=revision;status.busy=false;status.phase="ready".into();}
    }
}
#[tauri::command]
pub async fn edit_detail_open(state:State<'_,SharedState>,asset_id:String,library_id:String,recipe:serde_json::Value)->Result<DetailStatus,String>{
    let recipe=super::recipe::parse_recipe(&recipe)?;
    run_blocking(state.inner().clone(),move |state|{
        let (_,asset)=super::project::resolve(state,&library_id,&asset_id)?;
        let path=std::path::PathBuf::from(&asset.path);
        let id=uuid::Uuid::new_v4().to_string();
        let status=DetailStatus{id:id.clone(),phase:"queued".into(),width:0,height:0,revision:0,busy:true,error:None,cancelled:false};
        let job=Arc::new(Control{cancel:AtomicBool::new(false),status:Mutex::new(status.clone()),view:Mutex::new(None),image:Mutex::new(None),next:AtomicU64::new(0),touched:AtomicU64::new(now())});
        {let mut jobs=jobs().lock().map_err(|_|"细节作业锁不可用")?;for old in jobs.values(){old.cancel.store(true,Ordering::Release);}jobs.clear();jobs.insert(id.clone(),job.clone());}
        std::thread::Builder::new().name("editor-detail".into()).spawn(move ||{
            let result=super::export_worker::inspect(&path,&recipe,&job);
            if let Ok(mut status)=job.status.lock(){status.busy=false;if job.stopped(){status.cancelled=true;status.phase="cancelled".into();}else if let Err(error)=result{status.phase="error".into();status.error=Some(error);}};
        }).map_err(|e|{if let Ok(mut jobs)=jobs().lock(){jobs.remove(&id);}e.to_string()})?;
        Ok(status)
    }).await
}
#[tauri::command]
pub async fn edit_detail_status(job_id:String)->Result<DetailStatus,String>{let job=control(&job_id)?;let status=job.status.lock().map_err(|_|"细节状态锁不可用")?.clone();Ok(status)}
#[tauri::command]
pub async fn edit_detail_view(job_id:String,x:f64,y:f64,edge:u32,edges:bool)->Result<u64,String>{
    if !x.is_finite()||!y.is_finite()||!(0.0..=1.0).contains(&x)||!(0.0..=1.0).contains(&y)||!(64..=768).contains(&edge){return Err("细节视口无效".into());}
    let job=control(&job_id)?;let revision=job.next.fetch_add(1,Ordering::AcqRel)+1;
    *job.view.lock().map_err(|_|"细节视口锁不可用")?=Some(View{revision,x,y,edge,edges});
    job.status.lock().map_err(|_|"细节状态锁不可用")?.busy=true;Ok(revision)
}
#[tauri::command]
pub async fn edit_detail_image(job_id:String,revision:u64)->Result<tauri::ipc::Response,String>{
    let job=control(&job_id)?;
    let image=job.image.lock().map_err(|_|"细节图像锁不可用")?;
    let (_,bytes)=image.as_ref().filter(|(id,_)|*id==revision).ok_or("细节图像已过期")?;
    Ok(tauri::ipc::Response::new(bytes.clone()))
}
#[tauri::command]
pub async fn edit_detail_close(job_id:String)->Result<(),String>{
    if let Some(job)=jobs().lock().map_err(|_|"细节作业锁不可用")?.remove(&job_id){job.cancel.store(true,Ordering::Release);}Ok(())
}

#[derive(Serialize,Deserialize)]
pub(super) struct Ready {pub width:u32,pub height:u32}
#[derive(Serialize,Deserialize)]
pub(super) struct Frame {pub revision:u64,pub width:u32,pub height:u32}
fn write_json(path:&Path,value:&impl Serialize)->Result<(),String>{super::lut::atomic_write(path,&serde_json::to_vec(value).map_err(|e|e.to_string())?)}
/// This runs before Tauri initialization in the isolated worker process.
pub(super) fn worker(path:&Path,recipe:&super::recipe::EditRecipe,root:&Path,progress:&dyn Fn(&str))->Result<(),String>{
    let heartbeat=root.join("heartbeat");
    // A crashed parent must not leave a persistent inspector holding a full RAW.
    let watchdog=heartbeat.clone();std::thread::spawn(move ||loop{
        std::thread::sleep(Duration::from_secs(2));
        let alive=std::fs::metadata(&watchdog).ok().and_then(|m|m.modified().ok()).and_then(|t|t.elapsed().ok()).is_some_and(|age|age<Duration::from_secs(15));
        if !alive{std::process::exit(0);}
    });
    let effective=super::groups::effective(recipe);let recipe=effective.as_ref().unwrap_or(recipe);
    let doc=super::photocraft::prepare_export_document(path,recipe,None,progress)?;
    write_json(&root.join("ready.json"),&Ready{width:doc.size.width,height:doc.size.height})?;
    progress("ready");
    let mut previous=0;
    while heartbeat.exists(){
        let view=std::fs::read(root.join("view.json")).ok().and_then(|b|serde_json::from_slice::<View>(&b).ok());
        if let Some(view)=view.filter(|v|v.revision>previous){
            let (bytes,width,height)=render_region(&doc,&view)?;
            super::lut::atomic_write(&root.join(format!("frame-{}.png",view.revision)),&bytes)?;
            write_json(&root.join("frame.json"),&Frame{revision:view.revision,width,height})?;
            if previous>0{let _=std::fs::remove_file(root.join(format!("frame-{previous}.png")));}previous=view.revision;
        }
        std::thread::sleep(Duration::from_millis(40));
    }
    Ok(())
}
fn render_region(doc:&photocraft_engine::doc::Document,view:&View)->Result<(Vec<u8>,u32,u32),String>{
    use image::ImageEncoder;
    let width=view.edge.min(doc.size.width);let height=view.edge.min(doc.size.height);
    let x=((view.x*f64::from(doc.size.width)).round() as i64-i64::from(width)/2).clamp(0,i64::from(doc.size.width-width));
    let y=((view.y*f64::from(doc.size.height)).round() as i64-i64::from(height)/2).clamp(0,i64::from(doc.size.height-height));
    let pixels=photocraft_compose::render(doc,photocraft_engine::doc::Rect::from_xywh(x as i32,y as i32,width,height));
    let profile=photocraft_engine::color_cmds::document_profile(doc);
    if profile.channels()!=3{return Err("原尺寸细节检查目前需要 RGB 文档".into());}
    let mut rgb:Vec<_>=pixels.px.iter().flat_map(|p|[p[0],p[1],p[2]]).collect();
    photocraft_cms::cached(&profile,photocraft_cms::Builtin::Srgb.profile(),photocraft_cms::TransformOptions::default()).map_err(|e|e.to_string())?.apply(&mut rgb,3);
    if view.edges{
        let luminance:Vec<_>=rgb.chunks_exact(3).map(|p|0.2126*p[0]+0.7152*p[1]+0.0722*p[2]).collect();
        let smooth=photocraft_algo::matting::gaussian_blur(&luminance,width as usize,height as usize,1.0);
        for i in 0..smooth.len(){let (x,y)=(i%width as usize,i/width as usize);let gx=smooth[y*width as usize+(x+1).min(width as usize-1)]-smooth[y*width as usize+x.saturating_sub(1)];let gy=smooth[(y+1).min(height as usize-1)*width as usize+x]-smooth[y.saturating_sub(1)*width as usize+x];let edge=(gx.hypot(gy)*8.0).clamp(0.0,1.0);rgb[i*3..i*3+3].fill(edge);}
    }
    let bytes:Vec<_>=rgb.into_iter().map(|v|(v.clamp(0.0,1.0)*255.0).round() as u8).collect();
    let mut encoded=Vec::new();image::codecs::png::PngEncoder::new(&mut encoded).write_image(&bytes,width,height,image::ExtendedColorType::Rgb8).map_err(|e|e.to_string())?;
    Ok((encoded,width,height))
}
