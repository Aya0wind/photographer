//! Manual lens correction in a corrected source frame, before local painting.
//! Lensfun profiles are applied upstream in sensor development, before these manual controls.
use serde::{Serialize,Deserialize};
use photocraft_engine::doc::{Document,DocId,Layer,LayerContent};
#[derive(Clone,Debug,PartialEq,Serialize,Deserialize)]
#[serde(rename_all="camelCase",default)]
pub struct Settings {pub guide:Option<[f64;9]>,pub enabled:bool,pub distortion:f64,pub red_cyan:f64,pub blue_yellow:f64,pub vignette:f64,pub midpoint:f64,pub horizontal:f64,pub vertical:f64,pub scale:f64,pub purple:f64,pub green:f64,pub extend_edges:bool}
impl Default for Settings{fn default()->Self{Self{guide:None,enabled:true,distortion:0.0,red_cyan:0.0,blue_yellow:0.0,vignette:0.0,midpoint:50.0,horizontal:0.0,vertical:0.0,scale:100.0,purple:0.0,green:0.0,extend_edges:false}}}
impl Settings {
    fn active(&self)->bool{self.enabled&&(self.guide.is_some()||self.scale!=100.0||[self.distortion,self.red_cyan,self.blue_yellow,self.vignette,self.horizontal,self.vertical,self.purple,self.green].iter().any(|v|*v!=0.0))}
    fn params(&self)->photocraft_algo::lens::LensCorrection{photocraft_algo::lens::LensCorrection{distortion:self.distortion,red_cyan:self.red_cyan,blue_yellow:self.blue_yellow,vignette_amount:self.vignette,vignette_midpoint:self.midpoint,horizontal:self.horizontal,vertical:self.vertical,scale:self.scale,edge:if self.extend_edges{photocraft_algo::lens::EdgeMode::Extension}else{photocraft_algo::lens::EdgeMode::Transparency},..Default::default()}}
}
pub(super) fn normalize(value:&mut Settings)->Result<(),String>{
    if let Some(matrix)=&value.guide{if matrix.iter().any(|v|!v.is_finite()||v.abs()>100.0)||inverse(matrix).is_none(){return Err("引导透视矩阵无效".into());}}
    for (v,min,max) in [(&mut value.distortion,-100.0,100.0),(&mut value.red_cyan,-100.0,100.0),(&mut value.blue_yellow,-100.0,100.0),(&mut value.vignette,-100.0,100.0),(&mut value.midpoint,0.0,100.0),(&mut value.horizontal,-100.0,100.0),(&mut value.vertical,-100.0,100.0),(&mut value.scale,50.0,400.0),(&mut value.purple,0.0,100.0),(&mut value.green,0.0,100.0)]{if !v.is_finite(){return Err("镜头校正参数无效".into());}*v=v.clamp(min,max);}Ok(())
}
#[derive(Default)]
pub(super) struct Cache{key:Option<(DocId,Settings,f32)>,output:Option<Document>}
impl Cache {
    pub(super) fn apply(&mut self,base:&Document,value:Option<&Settings>,scale:f32)->Result<Document,String>{
        let Some(value)=value.filter(|s|s.active())else{return Ok(base.clone());};
        if self.key.as_ref().is_some_and(|(id,key,old_scale)|*id==base.id&&key==value&&*old_scale==scale){return self.output.clone().ok_or("镜头缓存不存在".into());}
        if base.mode!=photocraft_engine::doc::ColorMode::Rgb{return Err("镜头校正目前需要 RGB 文档".into());}
        let mut source=super::photocraft::processing_input(base)?;
        let LayerContent::Raster(surface)=&source.layers[0].content else{return Err("镜头来源缺少像素".into());};
        let frame=source.bounds();let map=photocraft_algo::lens::LensMap::new(&value.params(),frame);let guide=value.guide.as_ref().and_then(inverse);
        let mut surface=photocraft_algo::lens::remap(surface,frame,frame,value.params().edge,&|x,y|{
            let point=unproject(guide.as_ref(),base.size.width,base.size.height,[x,y])?;map.sample(point[0],point[1])
        });
        if value.purple>0.0||value.green>0.0{defringe(&mut surface,source.size.width,source.size.height,value,scale);}
        source.layers=vec![Layer::new("Corrected source",LayerContent::Raster(surface))];source.id=DocId::fresh();source.metadata=base.metadata.clone();
        self.key=Some((base.id,value.clone(),scale));self.output=Some(source.clone());Ok(source)
    }
}
fn defringe(surface:&mut photocraft_engine::doc::Surface,width:u32,height:u32,value:&Settings,scale:f32){
    let area=photocraft_engine::doc::Rect::from_xywh(0,0,width,height);let mut pixels=vec![[0.0;4];width as usize*height as usize];surface.read_rgba_into(area,&mut pixels);
    let light:Vec<_>=pixels.iter().map(|p|0.2126*p[0]+0.7152*p[1]+0.0722*p[2]).collect();
    let smooth=photocraft_algo::matting::gaussian_blur(&light,width as usize,height as usize,scale.max(0.5));
    for (i,p) in pixels.iter_mut().enumerate(){
        let (h,s,l)=photocraft_compose::adjust::rgb_to_hsl([p[0],p[1],p[2]]);let hue=h*360.0;
        let amount=if (230.0..=330.0).contains(&hue){value.purple}else if (60.0..=180.0).contains(&hue){value.green}else{0.0} as f32/100.0;
        let edge=((light[i]-smooth[i]).abs()*16.0).clamp(0.0,1.0);let rgb=photocraft_compose::adjust::hsl_to_rgb(h,s*(1.0-amount*edge),l);p[..3].copy_from_slice(&rgb);
    }
    let channels=surface.format().channels();let values:Vec<_>=pixels.iter().flat_map(|p|p[..channels].iter().copied()).collect();surface.write_region(area,&values);
}
#[tauri::command]
pub async fn edit_lens_autoscale(mut settings:Settings,width:u32,height:u32)->Result<f64,String>{
    normalize(&mut settings)?;if width==0||height==0||u64::from(width)*u64::from(height)>128_000_000{return Err("镜头图像尺寸无效".into());}
    let frame=photocraft_engine::doc::Rect::from_xywh(0,0,width,height);
    let guide=settings.guide.as_ref().and_then(inverse);
    let covers=|scale:f64|{let mut settings=settings.clone();settings.scale=scale;let map=photocraft_algo::lens::LensMap::new(&settings.params(),frame);
        (0..=40).all(|i|{let t=i as f64/40.0;[[0.5+t*f64::from(width-1),0.5],[0.5+t*f64::from(width-1),f64::from(height)-0.5],[0.5,0.5+t*f64::from(height-1)],[f64::from(width)-0.5,0.5+t*f64::from(height-1)]].iter().all(|p|unproject(guide.as_ref(),width,height,*p).and_then(|p|map.sample(p[0],p[1])).is_some_and(|sample|[sample.p,sample.pr,sample.pb].iter().all(|p|p[0]>=0.5&&p[1]>=0.5&&p[0]<=f64::from(width)-0.5&&p[1]<=f64::from(height)-0.5)))})};
    let mut low=settings.scale;let mut high=photocraft_algo::lens::auto_scale(&settings.params(),frame).clamp(low,400.0);
    while !covers(high)&&high<400.0{low=high;high=(high*1.2).min(400.0);}
    if !covers(high){return Err("自动缩放无法在 400% 以内完全覆盖，请手动裁剪".into());}
    for _ in 0..25{let mid=(low+high)/2.0;if covers(mid){high=mid;}else{low=mid;}}Ok(high)
}

pub(super) fn source_point(value:Option<&Settings>,width:u32,height:u32,point:[f64;2])->Result<[f64;2],String>{
    let Some(settings)=value.filter(|v|v.active())else{return Ok(point);};
    let frame=photocraft_engine::doc::Rect::from_xywh(0,0,width,height);
    let corrected=unproject(settings.guide.as_ref().and_then(inverse).as_ref(),width,height,[point[0]*f64::from(width),point[1]*f64::from(height)]).ok_or("取样位置在引导透视范围外")?;
    let sample=photocraft_algo::lens::LensMap::new(&settings.params(),frame).sample(corrected[0],corrected[1]).ok_or("取样位置在镜头校正范围外")?;
    let point=[sample.p[0]/f64::from(width),sample.p[1]/f64::from(height)];
    if point.iter().any(|v|!(0.0..=1.0).contains(v)){return Err("取样位置在原图范围外".into());}Ok(point)
}

fn inverse(m:&[f64;9])->Option<[f64;9]>{
    let values=[m[4]*m[8]-m[5]*m[7],m[2]*m[7]-m[1]*m[8],m[1]*m[5]-m[2]*m[4],m[5]*m[6]-m[3]*m[8],m[0]*m[8]-m[2]*m[6],m[2]*m[3]-m[0]*m[5],m[3]*m[7]-m[4]*m[6],m[1]*m[6]-m[0]*m[7],m[0]*m[4]-m[1]*m[3]];
    let determinant=m[0]*values[0]+m[1]*values[3]+m[2]*values[6];
    if !determinant.is_finite()||determinant.abs()<1e-8{return None;}Some(values.map(|v|v/determinant))
}
fn unproject(guide:Option<&[f64;9]>,width:u32,height:u32,point:[f64;2])->Option<[f64;2]>{
    let Some(m)=guide else{return Some(point);};let radius=f64::from(width).hypot(f64::from(height))/2.0;
    let x=(point[0]-f64::from(width)/2.0)/radius;let y=(point[1]-f64::from(height)/2.0)/radius;let divisor=m[6]*x+m[7]*y+m[8];
    if divisor<=0.05{return None;}Some([(m[0]*x+m[1]*y+m[2])/divisor*radius+f64::from(width)/2.0,(m[3]*x+m[4]*y+m[5])/divisor*radius+f64::from(height)/2.0])
}

#[derive(Clone,Debug,PartialEq,Serialize,Deserialize)]
#[serde(rename_all="camelCase")]
pub struct ProfileSettings {pub distortion:bool,pub tca:bool,pub auto_crop:bool,pub profile_id:String}
pub(super) fn validate_profile(value:&ProfileSettings)->Result<(),String>{if value.profile_id.len()>2048||value.profile_id.contains('\0'){return Err("镜头配置标识无效".into());}Ok(())}
#[tauri::command]
pub async fn edit_lens_profiles(state:tauri::State<'_,crate::ipc::SharedState>,asset_id:String,library_id:String,settings:Option<ProfileSettings>)->Result<serde_json::Value,String>{
    if let Some(value)=&settings{validate_profile(value)?;}
    crate::ipc::run_blocking(state.inner().clone(),move |state|{
        let (_,asset)=super::project::resolve(state,&library_id,&asset_id)?;
        let path=std::path::Path::new(&asset.path);
        if !super::rawlab::is_raw_path(path){return Err("镜头配置匹配目前支持 RAW 照片；其他照片可使用手动校正".into());}
        super::rawlab::lens_catalog(path,settings.as_ref())
    }).await
}
