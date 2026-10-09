import type { AdvancedAdjustments, EditRecipe, EditRecipeTextLayer } from "@/ipc/api";
import { defaultRecipe, initRecipeHistory, recipeReducer, type RecipeAction, type RecipeContext, type RecipeHistory } from "./recipe";

export const DEFAULT_ADVANCED: AdvancedAdjustments = {
  exposure: 0, temperature: 0, tint: 0, vibrance: 0, curves: [],
};

export function advancedRecipe(): EditRecipe {
  return { ...defaultRecipe(), renderer: "photocraft", advanced: { ...DEFAULT_ADVANCED } };
}

export type AdvancedAction = RecipeAction
  | { type: "advanced"; patch: Partial<AdvancedAdjustments>; record?: boolean }
  | { type: "loadProject"; recipe: EditRecipe }
  | { type: "textUpdateLive"; id: string; patch: Partial<EditRecipeTextLayer> };

export function advancedReducer(state: RecipeHistory, action: AdvancedAction, context: RecipeContext): RecipeHistory {
  if (action.type === "loadProject") return initRecipeHistory(action.recipe);
  if (action.type === "reset") return initRecipeHistory(advancedRecipe());
  if (action.type === "textUpdateLive") {
    const updated = recipeReducer(state, { type: "textUpdate", id: action.id, patch: action.patch }, context);
    return { ...updated, past: state.past };
  }
  if (action.type === "advanced") {
    const next = { ...state.present, advanced: { ...DEFAULT_ADVANCED, ...state.present.advanced, ...action.patch } };
    return { past: action.record === false ? state.past : [...state.past, state.present], present: next, future: [] };
  }
  return recipeReducer(state, action, context);
}
