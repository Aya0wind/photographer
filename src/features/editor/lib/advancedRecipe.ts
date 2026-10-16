import type { AdvancedAdjustments, DevelopmentAdjustments, EditLocalMask, EditRecipe, EditRecipeTextLayer } from "@/ipc/api";
import { appendHistory, defaultRecipe, initRecipeHistory, recipeEquals, recipeForPersist, recipeReducer, type RecipeAction, type RecipeContext, type RecipeHistory } from "./recipe";

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

export type AdjustmentGroup = "light" | "color" | "effects" | "detail" | "curves" | "levels" | "hsl" | "colorBalance" | "blackWhite" | "selectiveColor" | "colorGrading" | "parametricCurve";

export function adjustmentGroupChanged(recipe:EditRecipe,group:AdjustmentGroup):boolean {
  const a={...DEFAULT_ADVANCED,...recipe.advanced};
  const basic={brightness:0,contrast:0,saturation:0,...recipe.adjustments};
  switch(group) {
    case "parametricCurve":return !!a.parametricCurve;
    case "colorGrading":return !!a.colorGrading;
    case "colorBalance":return !!a.colorBalance;
    case "blackWhite":return !!a.blackWhite;
    case "selectiveColor":return !!a.selectiveColor;
    case "light":return !!(a.exposure||basic.brightness||basic.contrast||recipe.legacyAdjustments?.brightness||recipe.legacyAdjustments?.contrast)||developmentGroupChanged(a,"tone");
    case "effects":return developmentGroupChanged(a,"presence")||developmentGroupChanged(a,"effects");
    case "detail":return developmentGroupChanged(a,"detail");
    case "color":return !!(a.temperature||a.tint||a.vibrance||basic.saturation||recipe.legacyAdjustments?.saturation);
    case "curves":return !!a.curves.length || Object.values(a.channelCurves??{}).some(points=>!!points?.length);
    case "levels":return !!a.levels;
    case "hsl":return Object.keys(a.hsl??{}).length>0;
  }
}

export type AdvancedAction = RecipeAction
  | {type:"colorPipeline";value:EditRecipe["colorPipeline"];record?:boolean}
  | {type:"cropStart"}
  | {type:"cropConfirm";crop:EditRecipe["crop"]}
  | {type:"cropCancel"}
  | {type:"restoreHistory";history:RecipeHistory}
  | {type:"maskResetDevelopmentGroup";id:string;group:DevelopmentGroup}
  | {type:"resetDevelopmentGroup";group:DevelopmentGroup}
  | {type:"maskAdd";mask:EditLocalMask}
  | {type:"maskUpdate";id:string;patch:Partial<Omit<EditLocalMask,"id">>;record?:boolean}
  | {type:"maskReorder";id:string;targetId:string}
  | {type:"maskRemove";id:string}
  | {type:"maskDuplicate";id:string;newId:string;name:string}
  | {type:"maskResetAdjustmentGroup";id:string;group:AdjustmentGroup}
  | { type: "resetAdjustmentGroup"; group: AdjustmentGroup }
  | { type: "advanced"; patch: Partial<AdvancedAdjustments>; record?: boolean }
  | { type: "loadProject"; recipe: EditRecipe }
  | { type: "applyProject"; recipe: EditRecipe }
  | { type: "textUpdateLive"; id: string; patch: Partial<EditRecipeTextLayer> };

export function advancedReducer(state: RecipeHistory, action: AdvancedAction, context: RecipeContext): RecipeHistory {
  if(action.type==="restoreHistory")return action.history;
  if(action.type==="applyProject"){
    const before=state.cropSession??state;
    if(recipeEquals(before.present,action.recipe))return before;
    return {past:appendHistory(before.past,before.present),present:action.recipe,future:[]};
  }
  if(action.type==="cropStart")return state.cropSession?state:{...state,cropSession:{past:state.past,present:state.present,future:state.future}};
  if(action.type==="cropCancel")return state.cropSession??state;
  if(action.type==="cropConfirm"){
    const before=state.cropSession??{past:state.past,present:state.present,future:state.future};
    const next=recipeReducer({past:[],present:state.present,future:[]},{type:"cropApply",crop:action.crop},context).present;
    if(recipeEquals(recipeForPersist(before.present),recipeForPersist(next)))return before;
    return {past:appendHistory(before.past,before.present),present:next,future:[]};
  }
  if(state.cropSession&&action.type!=="reset"&&action.type!=="loadProject"){
    if(action.type==="undo"||action.type==="redo")return state.cropSession;
    const next=advancedReducer({past:state.past,present:state.present,future:state.future},action,context);
    return {...next,past:state.cropSession.past,future:state.cropSession.future,cropSession:state.cropSession};
  }
  if(action.type==="resetDevelopmentGroup"){
    const a=state.present.advanced??DEFAULT_ADVANCED;if(!developmentGroupChanged(a,action.group))return state;
    const development={...DEFAULT_DEVELOPMENT,...a.development};
    for(const key of DEVELOPMENT_GROUPS[action.group])Object.assign(development,{[key]:DEFAULT_DEVELOPMENT[key]});
    return {past:appendHistory(state.past,state.present),future:[],present:{...state.present,advanced:{...a,development}}};
  }
  if (action.type==="maskReorder" || action.type==="maskAdd" || action.type==="maskUpdate" || action.type==="maskRemove" || action.type==="maskDuplicate" || action.type==="maskResetAdjustmentGroup" || action.type==="maskResetDevelopmentGroup") {
    let masks=state.present.masks??[];
    let record=true;
    switch(action.type) {
      case "maskReorder":{
        const source=masks.find(m=>m.id===action.id),target=masks.find(m=>m.id===action.targetId);
        if(!source||!target||source===target)return state;
        const from=masks.indexOf(source),to=masks.indexOf(target);masks=[...masks];masks.splice(from,1);masks.splice(to,0,source);break;
      }
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
      case "maskResetDevelopmentGroup":masks=masks.map(m=>{
        if(m.id!==action.id)return m;
        const local={...state.present,masks:undefined,legacyAdjustments:undefined,adjustments:m.adjustments,advanced:m.advanced};
        const reset=advancedReducer(initRecipeHistory(local),{type:"resetDevelopmentGroup",group:action.group},context).present;
        return {...m,advanced:reset.advanced};
      });break;
    }
    const next={...state.present,masks:masks.length?masks:undefined};
    if(recipeEquals(next,state.present))return state;
    return {past:record?appendHistory(state.past,state.present):state.past,present:next,future:[]};
  }
  if (action.type === "resetAdjustmentGroup") {
    if(!adjustmentGroupChanged(state.present,action.group)) return state;
    const present=state.present;
    const advanced={...DEFAULT_ADVANCED,...present.advanced};
    const adjustments={brightness:0,contrast:0,saturation:0,...present.adjustments};
    function resetDevelopment(groups:DevelopmentGroup[]){
      if(!advanced.development)return;
      const development={...DEFAULT_DEVELOPMENT,...advanced.development};
      for(const group of groups)for(const key of DEVELOPMENT_GROUPS[group])Object.assign(development,{[key]:DEFAULT_DEVELOPMENT[key]});
      advanced.development=development;
    }
    switch(action.group) {
      case "parametricCurve":advanced.parametricCurve=undefined;break;
      case "colorGrading":advanced.colorGrading=undefined;break;
      case "colorBalance":advanced.colorBalance=undefined;break;
      case "blackWhite":advanced.blackWhite=undefined;break;
      case "selectiveColor":advanced.selectiveColor=undefined;break;
      case "light": advanced.exposure=0;adjustments.brightness=0;adjustments.contrast=0;resetDevelopment(["tone"]);break;
      case "effects":resetDevelopment(["presence","effects"]);break;
      case "detail":resetDevelopment(["detail"]);break;
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
    return {past:appendHistory(state.past,present),present:next,future:[]};
  }
  if (action.type === "loadProject") return initRecipeHistory(action.recipe);
  if (action.type === "reset") return initRecipeHistory({...advancedRecipe(),...(state.present.colorPipeline?.version===2?{colorPipeline:{...state.present.colorPipeline,film:undefined}}:{})});
  if (action.type === "textUpdateLive") {
    const updated = recipeReducer(state, { type: "textUpdate", id: action.id, patch: action.patch }, context);
    return { ...updated, past: state.past };
  }
  if(action.type==="colorPipeline"){const next={...state.present,colorPipeline:action.value};if(recipeEquals(next,state.present))return state;return {past:action.record===false?state.past:appendHistory(state.past,state.present),present:next,future:[]};}
  if (action.type === "advanced") {
    const next = { ...state.present, advanced: { ...DEFAULT_ADVANCED, ...state.present.advanced, ...action.patch } };
    if(recipeEquals(next,state.present)) return state;
    return { past: action.record === false ? state.past : appendHistory(state.past, state.present), present: next, future: [] };
  }
  return recipeReducer(state, action, context);
}
