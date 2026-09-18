import { describe, expect, it } from "vitest";

import { render, screen } from "@testing-library/react";
import { I18nextProvider } from "react-i18next";

import i18n from "@/i18n";
import PagePlaceholder from "./PagePlaceholder";

function renderPlaceholder(titleKey: string, descKey: string, children?: never) {
  return render(
    <I18nextProvider i18n={i18n}>
      <PagePlaceholder titleKey={titleKey} descKey={descKey}>
        {children}
      </PagePlaceholder>
    </I18nextProvider>,
  );
}

describe("PagePlaceholder", () => {
  it("按 i18n key 渲染标题与描述", () => {
    renderPlaceholder("pages.import.title", "pages.import.desc");

    // 标题为 h1，内容来自 zh.json
    expect(
      screen.getByRole("heading", { level: 1, name: "导入（M1 交付）" }),
    ).toBeInTheDocument();
    expect(screen.getByText("设备检测与导入向导将在里程碑 M1 于此交付。")).toBeInTheDocument();
  });

  it("不同页面的 key 渲染对应文案", () => {
    renderPlaceholder("pages.tasks.title", "pages.tasks.desc");

    expect(screen.getByRole("heading", { name: "任务中心（M5 交付）" })).toBeInTheDocument();
    expect(
      screen.getByText("导入与 AI 索引的后台任务队列将在里程碑 M1–M5 逐步交付。"),
    ).toBeInTheDocument();
  });

  it("children 附加内容渲染在卡片内", () => {
    render(
      <I18nextProvider i18n={i18n}>
        <PagePlaceholder titleKey="pages.settings.title" descKey="pages.settings.desc">
          <div>EXTRA_CHILDREN</div>
        </PagePlaceholder>
      </I18nextProvider>,
    );

    expect(screen.getByText("EXTRA_CHILDREN")).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "设置" })).toBeInTheDocument();
  });

  it("无 children 时仅渲染标题与描述", () => {
    renderPlaceholder("pages.gallery.title", "pages.gallery.desc");

    expect(screen.getByRole("heading", { name: "照片画廊（M4 交付）" })).toBeInTheDocument();
    expect(screen.queryByText("EXTRA_CHILDREN")).not.toBeInTheDocument();
  });
});
