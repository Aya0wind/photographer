import type {EditRecipe} from "@/ipc/api";
import {DEFAULT_ADVANCED,DEFAULT_DEVELOPMENT} from "./advancedRecipe";
export const TRANSFER_GROUPS=["light","color","curves","hsl","grading","detail","effects","profile","geometry","masks","annotations","output"] as const;
export type TransferGroup=typeof TRANSFER_GROUPS[number];
const ADVANCED={light:["exposure"],color:["temperature","tint","vibrance","colorBalance","blackWhite","selectiveColor"],curves:["curves","channelCurves","parametricCurve","levels"],hsl:["hsl","hslEight"],grading:["colorGrading","lookup"]} as const;
const DEVELOPMENT={light:["highlights","shadows","whites","blacks"],detail:["sharpenAmount","sharpenRadius","sharpenDetail","sharpenMasking","noiseLuminance","noiseLuminanceDetail","noiseColor","noiseColorDetail"],effects:["texture","clarity","dehaze","grainAmount","grainSize","grainRoughness","vignetteAmount","vignetteMidpoint","vignetteRoundness","vignetteFeather","vignetteHighlights","vignetteStyle"]} as const;
/** Copy only selected groups; absent source fields reset the corresponding target fields. */
export function transferRecipe(source:EditRecipe,target:EditRecipe,groups:TransferGroup[]):EditRecipe {
 const value=structuredClone(target);value.renderer="photocraft";
 const advanced={exposure:0,temperature:0,tint:0,vibrance:0,curves:[],...value.advanced};
 for(const group of groups){
  if(group in ADVANCED)for(const key of ADVANCED[group as keyof typeof ADVANCED])Object.assign(advanced,{[key]:structuredClone(source.advanced?.[key]??(DEFAULT_ADVANCED as Partial<typeof advanced>)[key])});
  if(group in DEVELOPMENT){const development={...DEFAULT_DEVELOPMENT,...advanced.development};for(const key of DEVELOPMENT[group as keyof typeof DEVELOPMENT])Object.assign(development,{[key]:source.advanced?.development?.[key]??DEFAULT_DEVELOPMENT[key]});advanced.development=development as typeof advanced.development;}
  if(group==="profile")value.colorPipeline=structuredClone(source.colorPipeline);
  if(group==="geometry"){value.crop=structuredClone(source.crop);value.rotateQuarter=source.rotateQuarter;value.geometry=structuredClone(source.geometry);}
  if(group==="masks")value.masks=structuredClone(source.masks);
  if(group==="annotations"){value.textLayers=structuredClone(source.textLayers);value.brushStrokes=structuredClone(source.brushStrokes);}
  if(group==="output")value.output=structuredClone(source.output);
 }
 // Keep basic and legacy exposure/saturation controls in the chosen group.
 for(const field of ["adjustments","legacyAdjustments"] as const){
  const patch={brightness:0,contrast:0,saturation:0,...value[field]};
  if(groups.includes("light")){patch.brightness=source[field]?.brightness??0;patch.contrast=source[field]?.contrast??0;}
  if(groups.includes("color"))patch.saturation=source[field]?.saturation??0;
  value[field]=patch;
 }
 value.advanced=advanced;return value;
}
