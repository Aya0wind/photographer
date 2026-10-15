import {afterEach,expect,it,vi} from "vitest";
import {getPreviewImage,preparePreviewImage,releasePreviewImage} from "./previewImages";
afterEach(()=>vi.unstubAllGlobals());
it("evicts cached frames without blanking image objects still displayed by the canvas",async()=>{
 class DecodedImage {src="";decoding="";onload=null;onerror=null;decode=vi.fn().mockResolvedValue(undefined);removeAttribute=vi.fn((name:string)=>{if(name==="src")this.src="";});}
 vi.stubGlobal("Image",DecodedImage);vi.stubGlobal("URL",{revokeObjectURL:vi.fn()});
 await preparePreviewImage("blob:frame");const shown=getPreviewImage("blob:frame")!;
 releasePreviewImage("blob:frame");
 expect(getPreviewImage("blob:frame")).toBeUndefined();expect(shown.src).toBe("blob:frame");expect(URL.revokeObjectURL).toHaveBeenCalledWith("blob:frame");
});
