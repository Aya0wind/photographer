import { useEffect, useState } from "react";
import { NavLink } from "react-router";
import { motion } from "motion/react";

import { useMotionOn } from "@/lib/motion";
import { useTranslation } from "react-i18next";

import type { ReactElement } from "react";

import { peopleList, sidebarCounts, subscribeAppEvents, type SidebarCounts } from "@/ipc/api";
import { useCullingStore } from "@/features/culling/cullingStore";
import { useSettingsStore } from "@/stores/settingsStore";

/**
 * 侧栏（M4.5 A3 信息架构重排，飞牛式分组）：浏览 / 组织 / 工具 / 系统四组。
 * - 浏览：图库、最近浏览（/recent）、那年今天（/memories，M7 F6）、
 *   收藏（F5 数据未就绪——禁用 +「即将支持」）
 * - 组织：相册（/albums，含手工相册和智能相册）、人物、器材统计（/gear，M7 F9）
 * - 工具：导入（任务并入右侧抽屉，M4.5）、相似照片（/similar，M7 F8 两级
 *   去重：完全重复 + pHash 近似，组内勾选清理）
 * - 系统：设置
 * 搜索已移除（TitleBar 全局搜索框承担；/search 路由保留）。
 * 计数徽标：图库/最近浏览/那年今天/相册走 sidebar_counts（一次性纯
 * COUNT，挂载拉一次 + 导入会话完成事件后重拉——最小事件集，不上轮询；
 * 0 或后端不可用不显示）；人物走 peopleList 聚类人脸总数（照旧）。
 */

/** sidebar_counts 键 → 徽标数据源 */
type CountBadge = Exclude<keyof SidebarCounts, never>;

interface NavItem {
  to: string;
  labelKey: string;
  icon: ReactElement;
  /** 徽标：people=聚类人脸总数；culling=进行中选片会话数；其余=sidebar_counts 对应键 */
  badge?: "people" | "culling" | CountBadge;
}

interface NavSection {
  titleKey: string;
  items: NavItem[];
}

function icon(path: ReactElement | ReactElement[], label: string): ReactElement {
  return (
    <svg
      viewBox="0 0 16 16"
      width="16"
      height="16"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.4"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
      className="shrink-0"
    >
      {path}
      <title>{label}</title>
    </svg>
  );
}

const ICONS = {
  gallery: icon(
    <>
      <rect x="1.5" y="2.5" width="13" height="11" rx="1.5" />
      <circle cx="5.5" cy="6" r="1.1" />
      <path d="M1.5 11l3.6-3.2a1 1 0 0 1 1.3 0L10.5 11.5M9 9.2l1.8-1.6a1 1 0 0 1 1.3 0l2.4 2.1" />
    </>,
    "gallery",
  ),
  recent: icon(
    <>
      <circle cx="8" cy="8" r="6" />
      <path d="M8 4.8V8l2.4 1.6" />
    </>,
    "recent",
  ),
  memories: icon(
    <>
      <rect x="2" y="3" width="12" height="10.5" rx="1.5" />
      <path d="M2 6.5h12M5.5 1.5v3M10.5 1.5v3" />
      <circle cx="8" cy="9.7" r="1.2" />
    </>,
    "memories",
  ),
  albums: icon(
    <>
      <rect x="1.5" y="3" width="13" height="10.5" rx="1.5" />
      <path d="M1.5 6h13M5 1.5h6" />
    </>,
    "albums",
  ),
  people: icon(
    <>
      <circle cx="8" cy="5.5" r="2.6" />
      <path d="M2.8 13.5c1-2.8 2.9-4.2 5.2-4.2s4.2 1.4 5.2 4.2" />
    </>,
    "people",
  ),
  gear: icon(
    <>
      <path d="M5.5 4.5l1-1.7h3l1 1.7" />
      <rect x="1.5" y="4.5" width="13" height="8.5" rx="1.5" />
      <circle cx="8" cy="8.6" r="2.4" />
    </>,
    "gear",
  ),
  map: icon(
    <>
      <circle cx="8" cy="8" r="6.2" />
      <path d="M1.8 8h12.4M8 1.8c1.7 1.8 2.6 3.9 2.6 6.2S9.7 12.4 8 14.2c-1.7-1.8-2.6-3.9-2.6-6.2S6.3 3.6 8 1.8z" />
    </>,
    "map",
  ),
  similar: icon(
    <>
      <rect x="2" y="2" width="7.5" height="7.5" rx="1" />
      <rect x="6.5" y="6.5" width="7.5" height="7.5" rx="1" />
    </>,
    "similar",
  ),
  culling: icon(
    <>
      <path d="M2 4.5h4l1.5 2H14" />
      <path d="M3.2 2.5h9.6l-1 4.2a3 3 0 0 1-2.9 2.3H7.1a3 3 0 0 1-2.9-2.3z" />
      <path d="M8 9v2.5" />
      <path d="M5.8 14l2.2-2.5L10.2 14" />
    </>,
    "culling",
  ),
  import: icon(
    <>
      <path d="M8 1.5v8M4.8 6.6L8 9.8l3.2-3.2" />
      <path d="M1.5 11v2a1.5 1.5 0 0 0 1.5 1.5h10a1.5 1.5 0 0 0 1.5-1.5v-2" />
    </>,
    "import",
  ),
  trash: icon(
    <>
      <path d="M3 4.5h10M6.5 2.5h3M4.5 4.5l.7 8.2a1.5 1.5 0 0 0 1.5 1.4h2.6a1.5 1.5 0 0 0 1.5-1.4l.7-8.2" />
      <path d="M6.7 7v4.4M9.3 7v4.4" />
    </>,
    "trash",
  ),
  settings: icon(
    <>
      <path d="M1.5 4.5h13M1.5 8h13M1.5 11.5h13" />
      <circle cx="5" cy="4.5" r="1.4" />
      <circle cx="10.8" cy="8" r="1.4" />
      <circle cx="6.2" cy="11.5" r="1.4" />
    </>,
    "settings",
  ),
};

/** 分组导航结构（浏览/组织/工具/系统）；badge=sidebar_counts 键 */
const SECTIONS: NavSection[] = [
  {
    titleKey: "nav.section.browse",
    items: [
      { to: "/gallery", labelKey: "nav.gallery", icon: ICONS.gallery, badge: "assets" },
      { to: "/recent", labelKey: "nav.recent", icon: ICONS.recent, badge: "recentViewed" },
    ],
  },
  {
    titleKey: "nav.section.organize",
    items: [
      { to: "/albums", labelKey: "nav.albums", icon: ICONS.albums, badge: "albums" },
      { to: "/memories", labelKey: "nav.memories", icon: ICONS.memories, badge: "onThisDay" },
      { to: "/people", labelKey: "nav.people", icon: ICONS.people, badge: "people" },
      { to: "/gear", labelKey: "nav.gear", icon: ICONS.gear },
      { to: "/map", labelKey: "nav.map", icon: ICONS.map },
    ],
  },
  {
    titleKey: "nav.section.tools",
    items: [
      { to: "/import", labelKey: "nav.import", icon: ICONS.import },
      { to: "/similar", labelKey: "nav.similar", icon: ICONS.similar },
      { to: "/culling", labelKey: "nav.culling", icon: ICONS.culling, badge: "culling" },
      { to: "/trash", labelKey: "nav.trash", icon: ICONS.trash },
    ],
  },
  {
    titleKey: "nav.section.system",
    items: [{ to: "/settings", labelKey: "nav.settings", icon: ICONS.settings }],
  },
];

export default function Sidebar() {
  const motionOn = useMotionOn();
  const { t } = useTranslation();
  // 计数/人脸徽标按库私有：切库时以 activeLibraryId 为依赖重拉
  const activeLibraryId = useSettingsStore((s) => s.settings.activeLibraryId);
  // 选片徽标：进行中会话数（cullingStore 集中维护，各入口建会话/收尾后刷新）
  const cullingActive = useCullingStore((s) => s.activeCount);
  const refreshCullingCount = useCullingStore((s) => s.refreshActiveCount);
  useEffect(() => {
    void refreshCullingCount();
  }, [refreshCullingCount, activeLibraryId]);
  // 人物入口徽标：人物数量（聚类的 person 条目数；进 app 拉一次；
  // 失败静默 0——后端未就绪不显示）
  const [peopleCount, setPeopleCount] = useState(0);
  useEffect(() => {
    let cancelled = false;
    void peopleList().then((list) => {
      if (cancelled) return;
      setPeopleCount(list.length);
    });
    return () => {
      cancelled = true;
    };
  }, [activeLibraryId]);

  // 计数徽标（sidebar_counts 一次性纯 COUNT）：挂载拉一次；导入会话完成后
  // 重拉（资产/标签/相册/最近浏览都可能变——最小事件集，不上轮询）。
  // 后端命令在途/不可用 → null，徽标不显示。
  const [counts, setCounts] = useState<SidebarCounts | null>(null);
  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | null = null;
    const pull = () => {
      void sidebarCounts().then((next) => {
        if (!cancelled && next !== null) setCounts(next);
      });
    };
    pull();
    void subscribeAppEvents((event) => {
      if (event.type === "importSessionFinished") pull();
    })
      .then((off) => {
        if (cancelled) off();
        else unlisten = off;
      })
      .catch(() => {
        // 非 Tauri 环境（vite dev 预览）静默
      });
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [activeLibraryId]);

  /** 行徽标值（people 走人物数量；culling 走进行中会话数；其余走 sidebar_counts；
   *  0/缺数据=不显示） */
  function badgeValueOf(badge: NonNullable<NavItem["badge"]>): number {
    if (badge === "people") return peopleCount;
    if (badge === "culling") return cullingActive;
    return counts ? counts[badge] : 0;
  }

  return (
    <aside className="ui-glass sp-scroll flex h-full w-[208px] shrink-0 flex-col overflow-y-auto border-r border-edge">
      <nav className="mt-3 flex flex-col gap-3 px-3 pb-4" aria-label="primary">
        {SECTIONS.map((section) => (
          <div key={section.titleKey} data-testid="nav-section" data-section={section.titleKey}>
            <h3 className="px-3.5 pb-1 pt-1.5 text-[10px] font-semibold uppercase tracking-wider text-text-muted">
              {t(section.titleKey)}
            </h3>
            <div className="flex flex-col gap-0.5">
              {section.items.map((item) => (
                <NavLink
                  key={item.to}
                  to={item.to}
                  className={({ isActive }: { isActive: boolean }) =>
                    [
                      "relative flex items-center gap-3 rounded-xl px-3.5 py-2.5 text-[13px] transition-colors duration-150",
                      isActive
                        ? "bg-accent/10 text-accent"
                        : "text-text-secondary hover:bg-panel/40 hover:text-text-primary",
                    ].join(" ")
                  }
                >
                  {({ isActive }: { isActive: boolean }) => (
                    <>
                      {isActive &&
                        (motionOn ? (
                          <motion.span
                            layoutId="sidebar-active-bar"
                            className="absolute left-0 top-1/2 h-4 w-[2.5px] -translate-y-1/2 rounded-full bg-accent"
                            aria-hidden="true"
                            transition={{ duration: 0.18, ease: "easeOut" }}
                          />
                        ) : (
                          <span
                            className="absolute left-0 top-1/2 h-4 w-[2.5px] -translate-y-1/2 rounded-full bg-accent"
                            aria-hidden="true"
                          />
                        ))}
                      {item.icon}
                      <span>{t(item.labelKey)}</span>
                      {item.badge && badgeValueOf(item.badge) > 0 && (
                        <span
                          className="ml-auto shrink-0 rounded-full bg-accent/15 px-1.5 py-0.5 font-mono text-[10px] leading-none tabular-nums text-accent"
                          data-testid={
                            item.badge === "people"
                              ? "sidebar-people-badge"
                              : item.badge === "culling"
                                ? "sidebar-culling-badge"
                                : "sidebar-count-badge"
                          }
                          data-kind={item.badge}
                        >
                          {badgeValueOf(item.badge)}
                        </span>
                      )}
                    </>
                  )}
                </NavLink>
              ))}
            </div>
          </div>
        ))}
      </nav>
    </aside>
  );
}
