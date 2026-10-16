import type {ExportOptions} from "@/ipc/api";
export interface EditorExportSettings {template:string;sequence:number;removeGps:boolean;author:string;copyright:string;keywords:string;finish:NonNullable<ExportOptions["finish"]>}
export const DEFAULT_EXPORT_SETTINGS:EditorExportSettings={template:"{name}_edited",sequence:1,removeGps:false,author:"",copyright:"",keywords:"",finish:{bitDepth:16,tiffCompression:"deflate",sharpen:0}};
export function exportName(name:string,format:NonNullable<ExportOptions["format"]>,settings:EditorExportSettings,now=new Date()):string {
 const replacements:Record<string,string>={name:name.replace(/\.[^.]+$/,""),date:now.toISOString().slice(0,10),sequence:String(settings.sequence).padStart(3,"0")};
 const stem=settings.template.replace(/\{(name|date|sequence)\}/g,(_,key:string)=>replacements[key]).trim();
 if(!stem||stem.length>180||/[<>:"/\\|?*\x00-\x1f{}]/.test(stem)||/[. ]$/.test(stem))throw new Error("Invalid export filename template");
 return `${stem}.${format==="jpeg"?"jpg":format==="tiff"?"tif":format}`;
}
export function exportMetadata(settings:EditorExportSettings):Pick<ExportOptions,"removeGps"|"author"|"copyright"|"keywords"|"finish"> {
 return {finish:settings.finish,removeGps:settings.removeGps,author:settings.author.trim(),copyright:settings.copyright.trim(),keywords:settings.keywords.split(/[,，]/).map(value=>value.trim()).filter(Boolean)};
}
