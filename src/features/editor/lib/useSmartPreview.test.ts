import {expect,it,vi} from "vitest";
import {prepareFrame} from "./useSmartPreview";
import {advancedRecipe} from "./advancedRecipe";
import {editPreviewActivate,editPreviewPrepare,editPreviewRender,type EditorPreviewSession} from "@/ipc/api";
import {preparePreviewImage} from "./previewImages";

vi.mock("@/ipc/api",()=>({editPreviewPrepare:vi.fn(),editPreviewActivate:vi.fn(),editPreviewRender:vi.fn()}));
vi.mock("./previewImages",()=>({preparePreviewImage:vi.fn(),releasePreviewImage:vi.fn()}));

it("publishes a prepared source even if sliders change during every image decode",async()=>{
 const latest={current:advancedRecipe()};
 const session={sessionId:"s",sourceUrl:"source",previewUrl:"activated"} as EditorPreviewSession;
 vi.mocked(editPreviewPrepare).mockResolvedValue(session);
 vi.mocked(editPreviewActivate).mockResolvedValue(session);
 vi.mocked(editPreviewRender).mockResolvedValue("catch-up");
 vi.mocked(preparePreviewImage).mockImplementation(async()=>{latest.current={...latest.current,advanced:{...latest.current.advanced!,exposure:latest.current.advanced!.exposure+.1}};});
 const result=await prepareFrame("s",latest,()=>true,()=>true,vi.fn());
 expect(result?.frame).toBe("catch-up");
 expect(editPreviewRender).toHaveBeenCalledTimes(1);
 expect(preparePreviewImage).toHaveBeenCalledTimes(2);
});
