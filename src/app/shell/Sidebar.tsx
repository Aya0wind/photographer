import { NavLink } from "react-router";
import { useTranslation } from "react-i18next";

import type { ReactElement } from "react";

interface NavItem {
  to: string;
  labelKey: string;
  icon: ReactElement;
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

const NAV_ITEMS: NavItem[] = [
  {
    to: "/gallery",
    labelKey: "nav.gallery",
    icon: icon(
      <>
        <rect x="1.5" y="2.5" width="13" height="11" rx="1.5" />
        <circle cx="5.5" cy="6" r="1.1" />
        <path d="M1.5 11l3.6-3.2a1 1 0 0 1 1.3 0L10.5 11.5M9 9.2l1.8-1.6a1 1 0 0 1 1.3 0l2.4 2.1" />
      </>,
      "gallery",
    ),
  },
  {
    to: "/search",
    labelKey: "nav.search",
    icon: icon(
      <>
        <circle cx="7" cy="7" r="4.5" />
        <path d="M10.5 10.5L14.5 14.5" />
      </>,
      "search",
    ),
  },
  {
    to: "/import",
    labelKey: "nav.import",
    icon: icon(
      <>
        <path d="M8 1.5v8M4.8 6.6L8 9.8l3.2-3.2" />
        <path d="M1.5 11v2a1.5 1.5 0 0 0 1.5 1.5h10a1.5 1.5 0 0 0 1.5-1.5v-2" />
      </>,
      "import",
    ),
  },
  {
    to: "/tasks",
    labelKey: "nav.tasks",
    icon: icon(
      <>
        <path d="M5.5 3.5h-2A1.5 1.5 0 0 0 2 5v7a1.5 1.5 0 0 0 1.5 1.5h9A1.5 1.5 0 0 0 14 12V5a1.5 1.5 0 0 0-1.5-1.5h-2" />
        <rect x="5.5" y="1.5" width="5" height="3" rx="1" />
        <path d="M4.8 8.6l1.4 1.4 2.6-2.6M11 8.5h1.5M11 11h1.5" />
      </>,
      "tasks",
    ),
  },
  {
    to: "/settings",
    labelKey: "nav.settings",
    icon: icon(
      <>
        <path d="M1.5 4.5h13M1.5 8h13M1.5 11.5h13" />
        <circle cx="5" cy="4.5" r="1.4" />
        <circle cx="10.8" cy="8" r="1.4" />
        <circle cx="6.2" cy="11.5" r="1.4" />
      </>,
      "settings",
    ),
  },
];

export default function Sidebar() {
  const { t } = useTranslation();

  return (
    <aside className="flex h-full w-[220px] shrink-0 flex-col border-r border-panel bg-surface">
      {/* 应用标识 */}
      <div className="flex h-14 items-center gap-2.5 px-5">
        <span className="h-2.5 w-2.5 rounded-full bg-accent" aria-hidden="true" />
        <span className="text-[15px] font-semibold tracking-wide text-text-primary">
          Smart Photo
        </span>
      </div>

      {/* 主导航 */}
      <nav className="mt-2 flex flex-col gap-0.5 px-2" aria-label="primary">
        {NAV_ITEMS.map((item) => (
          <NavLink
            key={item.to}
            to={item.to}
            className={({ isActive }: { isActive: boolean }) =>
              [
                "relative flex items-center gap-3 rounded-md px-3.5 py-2 text-sm transition-colors duration-150",
                isActive
                  ? "bg-panel/60 text-accent"
                  : "text-text-secondary hover:bg-panel/40 hover:text-text-primary",
              ].join(" ")
            }
          >
            {({ isActive }: { isActive: boolean }) => (
              <>
                {isActive && (
                  <span
                    className="absolute left-0 top-1/2 h-4 w-[2.5px] -translate-y-1/2 rounded-full bg-accent"
                    aria-hidden="true"
                  />
                )}
                {item.icon}
                <span>{t(item.labelKey)}</span>
              </>
            )}
          </NavLink>
        ))}
      </nav>
    </aside>
  );
}
