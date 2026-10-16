import {useEffect,useState} from "react";
import type {EditRecipe} from "@/ipc/api";

export type BaseProfile = NonNullable<NonNullable<EditRecipe["colorPipeline"]>["baseProfile"]>;
export interface EditorPreferences {
  defaultProfile:BaseProfile;
  leftOpen:boolean;
  rightOpen:boolean;
  leftWidth:number;
  rightWidth:number;
  background:"#202020"|"#101010"|"#404040";
}
const KEY="photographer.editor.preferences.v1";
const defaults:EditorPreferences={defaultProfile:"standard",leftOpen:true,rightOpen:true,leftWidth:224,rightWidth:320,background:"#202020"};

function read():EditorPreferences {
  try {
    const value=JSON.parse(localStorage.getItem(KEY)??"null");
    if(!value||typeof value!=="object")return defaults;
    return {
      defaultProfile:["standard","soft","neutral"].includes(value.defaultProfile)?value.defaultProfile:"standard",
      leftOpen:typeof value.leftOpen==="boolean"?value.leftOpen:true,
      rightOpen:typeof value.rightOpen==="boolean"?value.rightOpen:true,
      leftWidth:Number.isFinite(value.leftWidth)?Math.max(180,Math.min(340,value.leftWidth)):224,
      rightWidth:Number.isFinite(value.rightWidth)?Math.max(280,Math.min(480,value.rightWidth)):320,
      background:["#202020","#101010","#404040"].includes(value.background)?value.background:"#202020",
    };
  } catch {return defaults;}
}

export function useEditorPreferences() {
  const [preferences,setPreferences]=useState(read);
  useEffect(()=>{
    const sync=(event:StorageEvent)=>{if(event.key===KEY)setPreferences(read());};
    window.addEventListener("storage",sync);return()=>window.removeEventListener("storage",sync);
  },[]);
  function update(patch:Partial<EditorPreferences>) {
    setPreferences(before=>{
      const next={...before,...patch};
      try {localStorage.setItem(KEY,JSON.stringify(next));}catch{/* Preferences are optional; edits persist through the backend. */}
      return next;
    });
  }
  return {preferences,update};
}
