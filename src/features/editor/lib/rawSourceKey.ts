import type {EditRecipe} from "@/ipc/api";
/** Only sensor development settings invalidate the immutable linear preview. */
export const rawSourceKey=(recipe:EditRecipe)=>JSON.stringify([recipe.colorPipeline?.rawWhiteBalance??null,recipe.colorPipeline?.lensCorrection??null]);
