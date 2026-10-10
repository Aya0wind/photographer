import type { AdvancedAdjustments, DevelopmentAdjustments, EditLocalMask, EditRecipe, EditRecipeTextLayer } from "@/ipc/api";
import { defaultRecipe, initRecipeHistory, recipeEquals, recipeReducer, type RecipeAction, type RecipeContext, type RecipeHistory } from "./recipe";

export const DEFAULT_ADVANCED: AdvancedAdjustments = {
  exposure: 0, temperature: 0, tint: 0, vibrance: 0, curves: [],
};
export const DEFAULT_DEVELOPMENT:DevelopmentAdjustments={highlights:0,shadows:0,whites:0,blacks:0,texture:0,clarity:0,dehaze:0,sharpenAmount:0,sharpenRadius:1,sharpenDetail:25,sharpenMasking:0,noiseLuminance:0,noiseLuminanceDetail:50,noiseColor:0,noiseColorDetail:50,grainAmount:0,grainSize:25,grainRoughness:50,vignetteAmount:0,vignetteMidpoint:50,vignetteRoundness:0,vignetteFeather:50,vignetteHighlights:0,vignetteStyle:"highlightPriority"};
export const DEVELOPMENT_GROUPS={tone:["highlights","shadows","whites","blacks"],presence:["texture","clarity","dehaze"],detail:["sharpenAmount","sharpenRadius","sharpenDetail","sharpenMasking","noiseLuminance","noiseLuminanceDetail","noiseColor","noiseColorDetail"],effects:["grainAmount","grainSize","grainRoughness","vignetteAmount","vignetteMidpoint","vignetteRoundness","vignetteFeather","vignetteHighlights","vignetteStyle"]} as const;
export type DevelopmentGroup=keyof typeof DEVELOPMENT_GROUPS;
export function developmentActive(d:DevelopmentAdjustments|undefined){return !!d && [d.highlights,d.shadows,d.whites,d.blacks,d.texture,d.clarity,d.dehaze,d.sharpenAmount,d.noiseLuminance,d.noiseColor,d.grainAmount,d.vignetteAmount].some(v=>v!==0);}
export function developmentGroupChanged(a:AdvancedAdjustments,group:DevelopmentGroup){return !!a.development&&DEVELOPMENT_GROUPS[group].some(key=>a.development![key]!==DEFAULT_DEVELOPMENT[key]);}

export function advancedRecipe(): EditRecipe {
  return { ...defaultRecipe(), renderer: "photocraft", advanced: { ...DEFAULT_ADVANCED } };
}

export type AdjustmentGroup = "light" | "color" | "curves" | "levels" | "hsl" | "colorBalance" | "blackWhite" | "selectiveColor";

export function adjustmentGroupChanged(recipe:EditRecipe,group:AdjustmentGroup):boolean {
  const a={...DEFAULT_ADVANCED,...recipe.advanced};
  const basic={brightness:0,contrast:0,saturation:0,...recipe.adjustments};
  switch(group) {
    case "colorBalance":return !!a.colorBalance;
    case "blackWhite":return !!a.blackWhite;
    case "selectiveColor":return !!a.selectiveColor;
    case "light":return !!(a.exposure||basic.brightness||basic.contrast||recipe.legacyAdjustments?.brightness||recipe.legacyAdjustments?.contrast);
    case "color":return !!(a.temperature||a.tint||a.vibrance||basic.saturation||recipe.legacyAdjustments?.saturation);
    case "curves":return !!a.curves.length || Object.values(a.channelCurves??{}).some(points=>!!points?.length);
    case "levels":return !!a.levels;
    case "hsl":return Object.keys(a.hsl??{}).length>0;
  }
}

export type AdvancedAction = RecipeAction
  | {type:"resetDevelopmentGroup";group:DevelopmentGroup}
  | {type:"maskAdd";mask:EditLocalMask}
  | {type:"maskUpdate";id:string;patch:Partial<Omit<EditLocalMask,"id">>;record?:boolean}
  | {type:"maskRemove";id:string}
  | {type:"maskDuplicate";id:string;newId:string;name:string}
  | {type:"maskResetAdjustmentGroup";id:string;group:AdjustmentGroup}
  | { type: "resetAdjustmentGroup"; group: AdjustmentGroup }
  | { type: "advanced"; patch: Partial<AdvancedAdjustments>; record?: boolean }
  | { type: "loadProject"; recipe: EditRecipe }
  | { type: "textUpdateLive"; id: string; patch: Partial<EditRecipeTextLayer> };

export function advancedReducer(state: RecipeHistory, action: AdvancedAction, context: RecipeContext): RecipeHistory {
  if(action.type==="resetDevelopmentGroup"){
    const a=state.present.advanced??DEFAULT_ADVANCED;if(!developmentGroupChanged(a,action.group))return state;
    const development={...DEFAULT_DEVELOPMENT,...a.development};
    for(const key of DEVELOPMENT_GROUPS[action.group])Object.assign(development,{[key]:DEFAULT_DEVELOPMENT[key]});
    return {past:[...state.past,state.present],future:[],present:{...state.present,advanced:{...a,development}}};
  }
  if (action.type==="maskAdd" || action.type==="maskUpdate" || action.type==="maskRemove" || action.type==="maskDuplicate" || action.type==="maskResetAdjustmentGroup") {
    let masks=state.present.masks??[];
    let record=true;
    switch(action.type) {
      case "maskAdd":if(masks.some(m=>m.id===action.mask.id) || masks.length>=32)return state;masks=[...masks,structuredClone(action.mask)];break;
      case "maskUpdate":record=action.record!==false;masks=masks.map(m=>m.id===action.id?{...m,...action.patch}:m);break;
      case "maskRemove":masks=masks.filter(m=>m.id!==action.id);break;
      case "maskDuplicate":{
        const mask=masks.find(m=>m.id===action.id);
        if(!mask||masks.length>=32||masks.some(m=>m.id===action.newId))return state;
        masks=[...masks,{...structuredClone(mask),id:action.newId,name:action.name}];break;
      }
      case "maskResetAdjustmentGroup":masks=masks.map(m=>{
        if(m.id!==action.id)return m;
        const local={...state.present,masks:undefined,legacyAdjustments:undefined,adjustments:m.adjustments,advanced:m.advanced};
        const reset=advancedReducer(initRecipeHistory(local),{type:"resetAdjustmentGroup",group:action.group},context).present;
        return {...m,adjustments:reset.adjustments,advanced:reset.advanced};
      });break;
    }
    const next={...state.present,masks:masks.length?masks:undefined};
    if(recipeEquals(next,state.present))return state;
    return {past:record?[...state.past,state.present]:state.past,present:next,future:[]};
  }
  if (action.type === "resetAdjustmentGroup") {
    if(!adjustmentGroupChanged(state.present,action.group)) return state;
    const present=state.present;
    const advanced={...DEFAULT_ADVANCED,...present.advanced};
    const adjustments={brightness:0,contrast:0,saturation:0,...present.adjustments};
    switch(action.group) {
      case "colorBalance":advanced.colorBalance=undefined;break;
      case "blackWhite":advanced.blackWhite=undefined;break;
      case "selectiveColor":advanced.selectiveColor=undefined;break;
      case "light": advanced.exposure=0;adjustments.brightness=0;adjustments.contrast=0;break;
      case "color": advanced.temperature=0;advanced.tint=0;advanced.vibrance=0;adjustments.saturation=0;break;
      case "curves": advanced.curves=[];advanced.channelCurves={};break;
      case "levels": advanced.levels=undefined;break;
      case "hsl": advanced.hsl={};break;
    }
    let legacyAdjustments=present.legacyAdjustments;
    if(legacyAdjustments&&action.group==="light")legacyAdjustments={...legacyAdjustments,brightness:0,contrast:0};
    if(legacyAdjustments&&action.group==="color")legacyAdjustments={...legacyAdjustments,saturation:0};
    const next={...present,advanced,adjustments,...(present.legacyAdjustments?{legacyAdjustments}:{})};
    if(recipeEquals(next,present)) return state;
    return {past:[...state.past,present],present:next,future:[]};
  }
  if (action.type === "loadProject") return initRecipeHistory(action.recipe);
  if (action.type === "reset") return initRecipeHistory(advancedRecipe());
  if (action.type === "textUpdateLive") {
    const updated = recipeReducer(state, { type: "textUpdate", id: action.id, patch: action.patch }, context);
    return { ...updated, past: state.past };
  }
  if (action.type === "advanced") {
    const next = { ...state.present, advanced: { ...DEFAULT_ADVANCED, ...state.present.advanced, ...action.patch } };
    if(recipeEquals(next,state.present)) return state;
    return { past: action.record === false ? state.past : [...state.past, state.present], present: next, future: [] };
  }
  return recipeReducer(state, action, context);
}
