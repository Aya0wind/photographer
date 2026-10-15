import {useLayoutEffect, useReducer, useRef} from "react";
import {afterEach, beforeEach, expect, it, vi} from "vitest";
import {act, fireEvent, render, screen} from "@testing-library/react";
import {I18nextProvider} from "react-i18next";
import i18n from "@/i18n";
import ColorWheel from "./ColorWheel";
import CurveEditor from "./CurveEditor";
import Slider from "./AdjustmentSlider";
import {advancedRecipe, advancedReducer, type AdvancedAction} from "../lib/advancedRecipe";
import {initRecipeHistory, recipeEquals, type RecipeHistory} from "../lib/recipe";
import {DEFAULT_GRADING} from "./ColorGradingPanel";

type Control = "curve" | "wheel" | "slider";
beforeEach(() => vi.stubGlobal("PointerEvent", class extends MouseEvent {
  pointerId: number;
  constructor(type: string, options: PointerEventInit = {}) {
    super(type, options); this.pointerId = options.pointerId ?? 1;
  }
}));
afterEach(() => vi.unstubAllGlobals());

function Harness({control, clone = false}: {control: Control; clone?: boolean}) {
  const [history, dispatch] = useReducer((state: RecipeHistory, action: AdvancedAction) =>
    advancedReducer(state, action, {width: 600, height: 400}), undefined, () => {
      const recipe = advancedRecipe();
      recipe.advanced!.curves = [[0, 0], [128, 128], [255, 255]];
      recipe.advanced!.colorGrading = structuredClone(DEFAULT_GRADING);
      return initRecipeHistory(recipe);
    });
  const snapshot = useRef<RecipeHistory | null>(null);
  const frame = useRef<HTMLDivElement>(null);
  const interleaved = useRef(clone);
  // A second movement arrives after a render, before its passive effects flush.
  // This catches effects comparing an older prop with an already advanced ref.
  useLayoutEffect(() => {
    if (!snapshot.current || history.present === snapshot.current.present || interleaved.current) return;
    interleaved.current = true;
    frame.current!.querySelector('[role="application"],input[type="range"]')!.dispatchEvent(
      new PointerEvent("pointermove", {bubbles: true, clientX: 160, clientY: 80, altKey: control === "slider"}),
    );
  }, [history, control]);
  const begin = () => { snapshot.current ??= history; };
  const end = () => {
    if (snapshot.current && !recipeEquals(snapshot.current.present, history.present))
      dispatch({type: "commitFrom", snapshot: snapshot.current.present});
    snapshot.current = null;
  };
  const cancel = () => {
    if (snapshot.current) dispatch({type: "restoreHistory", history: snapshot.current});
    snapshot.current = null;
  };
  const a = clone ? structuredClone(history.present.advanced!) : history.present.advanced!;
  return <I18nextProvider i18n={i18n}><div ref={frame}>
    {control === "curve" && <CurveEditor adjustments={a} onChange={(patch, record) => dispatch({type: "advanced", patch, record})}
      onReset={() => {}} begin={begin} end={end} sampling={{available: false, picker: null, setPicker: () => {}, sample: null, histogram: []}}/>}
    {control === "wheel" && <ColorWheel label="Wheel" value={a.colorGrading!.midtones}
      onChange={(value, record) => dispatch({type: "advanced", patch: {colorGrading: {...a.colorGrading!, midtones: value}}, record})}
      begin={begin} end={end} cancel={cancel}/>}
    {control === "slider" && <Slider label="Exposure" value={a.exposure} min={-5} max={5} step={.1}
      onChange={exposure => dispatch({type: "advanced", patch: {exposure}, record: false})}
      onSet={() => {}} onReset={() => {}} begin={begin} end={end}/>}
  </div><button onClick={cancel}>Cancel</button><button onClick={() => dispatch({type: "undo"})}>Undo</button>
    <output data-testid="state">{JSON.stringify(history)}</output></I18nextProvider>;
}
const state = () => JSON.parse(screen.getByTestId("state").textContent!) as RecipeHistory;
function setup(control: Control, clone = false) {
  render(<Harness control={control} clone={clone}/>);
  const target = screen.getByRole(control === "slider" ? "slider" : "application");
  target.getBoundingClientRect = () => new DOMRect(0, 0, 255, 255);
  target.setPointerCapture = vi.fn();
  return target;
}
for (const control of ["curve", "wheel", "slider"] as const) {
  it(`keeps ${control} dragging through interleaved renders and commits one undo`, () => {
    const target = setup(control);
    const initial = state().present;
    fireEvent.pointerDown(target, {button: 0, clientX: 128, clientY: 127, altKey: control === "slider"});
    fireEvent.pointerMove(target, {clientX: 140, clientY: 100, altKey: control === "slider"});
    const before = state().present;
    act(() => {
      target.dispatchEvent(new PointerEvent("pointermove", {bubbles: true, clientX: 190, clientY: 60, altKey: control === "slider"}));
    });
    expect(recipeEquals(state().present, before)).toBe(false);
    expect(state().past).toHaveLength(0);
    fireEvent.pointerUp(target);
    expect(state().past).toHaveLength(1);
    fireEvent.click(screen.getByText("Undo"));
    expect(state().present).toEqual(initial);
  });
}
for (const control of ["curve", "wheel"] as const) {
  it(`keeps ${control} dragging when the parent copies equal parameter values`, () => {
    const target = setup(control, true);
    fireEvent.pointerDown(target, {button: 0, clientX: 128, clientY: 127});
    fireEvent.pointerMove(target, {clientX: 140, clientY: 100});
    const before = state().present;
    fireEvent.pointerMove(target, {clientX: 190, clientY: 60});
    expect(recipeEquals(state().present, before)).toBe(false);
    fireEvent.click(screen.getByText("Cancel"));
    const cancelled = state();
    fireEvent.pointerMove(target, {clientX: 210, clientY: 40});
    fireEvent.pointerUp(target);
    expect(state()).toEqual(cancelled);
  });
}

it("keeps a curve point movable after clamping at an edge and ignores movement after release", () => {
  const target = setup("curve", true);
  fireEvent.pointerDown(target, {button: 0, clientX: 128, clientY: 127});
  fireEvent.pointerMove(target, {clientX: 500, clientY: -200});
  expect(state().present.advanced!.curves[1]).toEqual([254, 255]);
  fireEvent.pointerMove(target, {clientX: 450, clientY: -150});
  expect(state().present.advanced!.curves[1]).toEqual([204, 205]);
  fireEvent.pointerUp(target);
  const released = state();
  fireEvent.pointerMove(target, {clientX: 200, clientY: 120});
  expect(state()).toEqual(released);
  expect(state().past).toHaveLength(1);
});
