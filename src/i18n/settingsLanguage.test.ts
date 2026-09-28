import { afterEach, expect, it, vi } from "vitest";
import { waitFor } from "@testing-library/react";
import i18n, { setAppLanguage } from "./index";
import { initAppLanguage } from "./settingsLanguage";
import { clone, DEFAULT_SETTINGS, useSettingsStore } from "@/stores/settingsStore";

const ipcMock = vi.hoisted(() => vi.fn());
vi.mock("@/ipc", () => ({ ipc: ipcMock }));
let stop: (() => void) | undefined;
afterEach(async () => {
  stop?.(); stop = undefined;
  useSettingsStore.setState({ settings: clone(DEFAULT_SETTINGS) });
  await setAppLanguage('zh');
});

it("启动恢复已保存的语言，远端设置加载和后续切换都更新界面", async () => {
  const saved = clone(DEFAULT_SETTINGS);
  saved.system.language = 'es';
  ipcMock.mockResolvedValue(saved);
  stop = initAppLanguage();
  await useSettingsStore.getState().load();
  await waitFor(() => expect(i18n.language).toBe('es'));
  useSettingsStore.getState().update({ system: { language: 'ja' } });
  await waitFor(() => expect(i18n.language).toBe('ja'));
  expect(i18n.t('common.cancel')).toBe('キャンセル');
});
