//! Replay non-destructive retouch strokes through pinned PhotoCraft commands.
use super::{photocraft,recipe::RetouchStroke};
use photocraft_engine::{Session,doc::{DocId,Document}};
use serde_json::json;
#[derive(Default)]
pub(super) struct RetouchCache {source:Option<DocId>,strokes:Vec<RetouchStroke>,output:Option<Document>}
impl RetouchCache {
 pub fn apply(&mut self,base:&Document,strokes:&[RetouchStroke])->Result<Document,String>{
  if strokes.is_empty(){return Ok(base.clone());}
  if self.source!=Some(base.id)||!strokes.starts_with(&self.strokes){let output=photocraft::processing_input(base)?;self.source=Some(base.id);self.strokes.clear();self.output=Some(output);}
  if self.strokes==strokes{return self.output.clone().ok_or("修复来源不存在".into());}
  let mut session=Session::new();session.add_document(self.output.clone().ok_or("修复来源不存在")?,None);
  for stroke in &strokes[self.strokes.len()..]{
   if stroke.opacity==0.0 {continue;}
   let points:Vec<_>=stroke.points.iter().map(|p|json!([p.x*f64::from(base.size.width),p.y*f64::from(base.size.height),1.0])).collect();
   let command=match stroke.kind.as_str(){"clone"=>"paint.cloneStamp","heal"=>"paint.healingBrush","redEye"=>"paint.redEye",_=>return Err("修复模式无效".into())};
   let params=if stroke.kind=="redEye"{
    let point=stroke.points.first().ok_or("红眼位置缺失")?;
    json!({"x":point.x*f64::from(base.size.width),"y":point.y*f64::from(base.size.height),"pupilSize":((stroke.width_rel*f64::from(base.size.width.min(base.size.height))/2.0-4.0)/1.2).clamp(1.0,100.0),"darken":stroke.opacity*100.0})
   }else{json!({"points":points,"source":[stroke.source.x*f64::from(base.size.width),stroke.source.y*f64::from(base.size.height)],"aligned":false,"sampleLayer":"current","size":(stroke.width_rel*f64::from(base.size.width.min(base.size.height))).clamp(0.5,5000.0),"hardness":stroke.hardness*100.0,"opacity":stroke.opacity*100.0,"flow":100.0,"spacing":15.0})};
   if let Err(error)=photocraft::execute(&mut session,command,params){if stroke.kind!="redEye"||!error.contains("no red-eye pixels"){self.source=None;return Err(error);}}
   session.journal.clear();session.active_mut().ok_or("修复文档不存在")?.history=Default::default();
  }
  let mut output=photocraft::active_document(&session)?;output.id=DocId::fresh();self.output=Some(output.clone());self.strokes=strokes.to_vec();Ok(output)
 }
}

pub(super) fn normalize(strokes:&mut [RetouchStroke],renderer:Option<super::recipe::RenderEngine>)->Result<(),String>{
    if strokes.len()>256{return Err("修复笔迹最多 256 笔".into());}
    let mut retouch_ids=std::collections::HashSet::new();let mut retouch_points=0usize;
    for stroke in strokes{
        retouch_points=retouch_points.saturating_add(stroke.points.len());
        if !["clone","heal","redEye"].contains(&stroke.kind.as_str())||stroke.id.is_empty()||stroke.id.len()>96||!retouch_ids.insert(stroke.id.clone())||stroke.points.is_empty()||retouch_points>100000{return Err("修复笔迹无效或过多".into());}
        if renderer!=Some(super::recipe::RenderEngine::Photocraft){return Err("修复需要高级编辑引擎".into());}
        for point in std::iter::once(&mut stroke.source).chain(stroke.points.iter_mut()){
            if !point.x.is_finite()||!point.y.is_finite(){return Err("修复坐标无效".into());}point.x=point.x.clamp(0.0,1.0);point.y=point.y.clamp(0.0,1.0);
        }
        for (value,min,max) in [(&mut stroke.width_rel,0.0001,0.5),(&mut stroke.hardness,0.0,1.0),(&mut stroke.opacity,0.0,1.0)]{
            if !value.is_finite(){return Err("修复画笔参数无效".into());}*value=value.clamp(min,max);
        }
    }
    Ok(())
}
