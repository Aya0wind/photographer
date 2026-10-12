import {listen} from "@tauri-apps/api/event";
import {photoLibraryReorganize,type PhotoLibraryOrganizationProgress} from "@/ipc/api";
export async function organizeLibrary(id:string,onProgress:(progress:PhotoLibraryOrganizationProgress)=>void){
 onProgress({libraryId:id,phase:"scan",completed:0,total:0});
 const release=await listen<PhotoLibraryOrganizationProgress>("photo-library-organizing",event=>{if(event.payload.libraryId===id)onProgress(event.payload);}).catch(()=>()=>{});
 try{return await photoLibraryReorganize(id);}finally{release();}
}
