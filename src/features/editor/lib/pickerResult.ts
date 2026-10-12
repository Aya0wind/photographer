import type {AdvancedAdjustments,editPreviewPick} from "@/ipc/api";
/** Eyedroppers return both a readout and curve corrections. A readout must
 * never suppress applying black/gray/white point adjustments. */
export function pickerAdjustment(picker:string,result:Awaited<ReturnType<typeof editPreviewPick>>,advanced:AdvancedAdjustments|undefined):Pick<AdvancedAdjustments,"curves"|"channelCurves">|null{
 if(!["black","gray","white"].includes(picker))return null;
 return {curves:result.points??[],channelCurves:{...advanced?.channelCurves,red:result.red??[],green:result.green??[],blue:result.blue??[]}};
}
