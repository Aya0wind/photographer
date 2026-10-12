//! 照片库组织规则：安全的相对路径模板、时间归一和重名处理。
//! 仅由软件主动写入新文件时使用；扫描已有文件不调用模板、不搬动文件。

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};

pub const DEFAULT_ORGANIZATION_TEMPLATE:&str="{year}/{month}/{day}/{filename}";
pub fn validate_photo_folder(root:&Path,existing:bool)->Result<(),String>{
    if existing{if !root.is_dir(){return Err("所选照片文件夹不存在或不可访问".into());}}
    else if root.exists(){let mut entries=std::fs::read_dir(root).map_err(|e|e.to_string())?;if entries.next().is_some(){return Err("新建照片文件夹必须为空；已有照片请选择使用现有文件夹".into());}}
    Ok(())
}
fn variable(name:&str)->Option<&'static str>{Some(match name{
    "year"|"年"=>"year","month"|"月"=>"month","day"|"日"=>"day","hour"|"时"=>"hour","minute"|"分"=>"minute","second"|"秒"=>"second",
    "camera"|"相机"=>"camera","filename"|"原文件名"=>"filename","stem"|"文件主名"=>"stem","ext"|"扩展名"=>"ext",_=>return None})}
pub fn validate_organization_template(template:&str)->Result<(),String>{
    if template.trim().is_empty()||template.len()>512||template.starts_with('/')||template.starts_with('\\')||template.contains('\\')||template.contains(':'){return Err("组织模板必须是照片库内的相对路径".into());}
    let mut skeleton=String::new();let mut rest=template;let mut filename=false;let mut stem=false;let mut ext=false;
    while let Some(start)=rest.find('{'){
        let literal=&rest[..start];if literal.contains('}'){return Err("模板括号不完整".into());}skeleton.push_str(literal);
        let end=rest[start+1..].find('}').ok_or("模板括号不完整")?+start+1;let key=variable(&rest[start+1..end]).ok_or("模板包含未知变量")?;
        filename|=key=="filename";stem|=key=="stem";ext|=key=="ext";skeleton.push('x');rest=&rest[end+1..];
    }
    if rest.contains('}'){return Err("模板括号不完整".into());}skeleton.push_str(rest);
    if !filename&&!(stem&&ext){return Err("模板必须保留原文件名，或同时包含文件主名和扩展名".into());}
    if skeleton.split('/').any(|s|s.is_empty()||s=="."||s==".."||s.chars().any(is_illegal)||s.ends_with('.')||s.ends_with(' ')){return Err("模板包含无效目录或文件名".into());}
    Ok(())
}
pub fn organized_rel(template:&str,captured:DateTime<Utc>,filename:&str,camera:Option<&str>)->Result<String,String>{
    validate_organization_template(template)?;let file=Path::new(filename);let stem=file.file_stem().and_then(|s|s.to_str()).unwrap_or(filename);let ext=file.extension().and_then(|s|s.to_str()).unwrap_or("");
    let mut out=String::new();let mut rest=template;
    while let Some(start)=rest.find('{'){
        out.push_str(&rest[..start]);let end=rest[start+1..].find('}').unwrap()+start+1;
        let value=match variable(&rest[start+1..end]).unwrap(){"year"=>captured.format("%Y").to_string(),"month"=>captured.format("%m").to_string(),"day"=>captured.format("%d").to_string(),"hour"=>captured.format("%H").to_string(),"minute"=>captured.format("%M").to_string(),"second"=>captured.format("%S").to_string(),"filename"=>filename.into(),"stem"=>stem.into(),"ext"=>ext.into(),"camera"=>camera.filter(|s|!s.trim().is_empty()&&s.trim()!="---").unwrap_or("Unknown camera").into(),_=>unreachable!()};
        out.push_str(&sanitize_component(&value).replace(['/', '\\'],"_").trim_end_matches(['.',' ']).to_string());rest=&rest[end+1..];
    }
    out.push_str(rest);
    for segment in out.split('/'){
        let name=segment.split('.').next().unwrap_or("").to_ascii_uppercase();
        if segment.is_empty()||segment=="."||segment==".."||["CON","PRN","AUX","NUL"].contains(&name.as_str())||(name.len()==4&&(name.starts_with("COM")||name.starts_with("LPT"))&&matches!(name.as_bytes()[3],b'1'..=b'9')){return Err("模板生成了无效的文件路径".into());}
    }
    if !Path::new(&out).extension().and_then(|s|s.to_str()).unwrap_or("").eq_ignore_ascii_case(ext){return Err("模板不能改变照片文件的扩展名".into());}
    Ok(out)
}

/// 拍摄时间归一：EXIF DateTimeOriginal 优先，缺失回退文件 mtime。
/// 纯时间布局的年/月段与 `assets.captured_at` 落库共用此口径。
pub fn resolve_captured(exif_at: Option<DateTime<Utc>>, mtime: DateTime<Utc>) -> DateTime<Utc> {
    exif_at.unwrap_or(mtime)
}

/// Windows 文件名段清理：非法字符 `:*?"<>|` 与控制字符替换为 `_`。
pub fn sanitize_component(s: &str) -> String {
    s.chars()
        .map(|c| if is_illegal(c) { '_' } else { c })
        .collect()
}

/// 冲突递增的唯一路径（纯计算，不创建文件）：`a.txt` 被占用 → `a_1.txt`、
/// `a_2.txt`…；`dir` 不存在或无冲突时直接 `join`。导入重名策略的唯一实现
/// （§三「重名沿用现有 rename 策略」）。
pub fn unique_path(dir: &Path, name: &str) -> PathBuf {
    let first = dir.join(name);
    if !first.exists() {
        return first;
    }
    // 按最后一个点切分扩展名（保留大小写）；无扩展名或点开头的主名为空则整体视作主名
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s, format!(".{e}")),
        _ => (name, String::new()),
    };
    for i in 1u32.. {
        let candidate = dir.join(format!("{stem}_{i}{ext}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    unreachable!("u32 序列不可能全部被占用")
}

fn is_illegal(c: char) -> bool {
    matches!(c, ':' | '*' | '?' | '"' | '<' | '>' | '|') || c.is_control()
}

#[cfg(test)]
mod tests {
 use super::*;
 #[test]fn folder_choice_does_not_move_existing_files_and_rejects_nonempty_new_folder(){let dir=tempfile::tempdir().unwrap();let photo=dir.path().join("old.jpg");std::fs::write(&photo,b"original").unwrap();assert!(validate_photo_folder(dir.path(),true).is_ok());assert!(validate_photo_folder(dir.path(),false).is_err());assert!(validate_photo_folder(&dir.path().join("missing"),true).is_err());assert_eq!(std::fs::read(photo).unwrap(),b"original");}
 #[test]fn renders_library_templates_and_keeps_raw_jpeg_stems_together(){
    let at=DateTime::parse_from_rfc3339("2026-10-11T14:30:45Z").unwrap().with_timezone(&Utc);
    assert_eq!(organized_rel(DEFAULT_ORGANIZATION_TEMPLATE,at,"DSC.ARW",None).unwrap(),"2026/10/11/DSC.ARW");
    assert_eq!(organized_rel("{年}/{月}/{日}/{文件主名}_{时}{分}{秒}.{扩展名}",at,"DSC.ARW",None).unwrap(),"2026/10/11/DSC_143045.ARW");
    assert_eq!(organized_rel("{year}/{stem}_{hour}.{ext}",at,"DSC.JPG",None).unwrap(),"2026/DSC_14.JPG");
    assert_eq!(organized_rel("{camera}/{filename}",at,"DSC.ARW",Some("---")).unwrap(),"Unknown camera/DSC.ARW");
 }
 #[test]fn rejects_escape_reserved_names_unknown_variables_and_extension_changes(){
    let at=Utc::now();for template in ["../{filename}","/{filename}","C:/{filename}","{unknown}/{filename}","{year}//{filename}","CON/{filename}","{stem}.jpg"]{assert!(organized_rel(template,at,"image.ARW",None).is_err(),"{template}");}
 }
}
