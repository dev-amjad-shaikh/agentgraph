import { Link, NavLink, Outlet } from "react-router";
import { Flame, Github } from "lucide-react";

const navItems = [
  { to: "/", label: "Overview", end: true },
  { to: "/learn", label: "Learn", end: false },
  { to: "/playground", label: "Playground", end: false },
];

/**
 * Site-wide shell: sticky header nav + footer. Routed pages render in <Outlet />.
 */
export function SiteLayout() {
  return (
    <div className="flex min-h-screen flex-col">
      <header className="sticky top-0 z-50 border-b bg-background/85 backdrop-blur">
        <div className="mx-auto flex h-14 max-w-6xl items-center justify-between px-4 sm:px-6">
          <Link to="/" className="flex items-center gap-2">
            <span className="flex h-7 w-7 items-center justify-center rounded-lg bg-primary text-primary-foreground">
              <Flame size={15} strokeWidth={2.4} />
            </span>
            <span className="font-display text-lg font-semibold tracking-tight">
              rusty
            </span>
            <span className="hidden font-code text-[10px] text-muted-foreground sm:inline">
              aboutrusty.com
            </span>
          </Link>
          <nav className="flex items-center gap-1 sm:gap-2">
            {navItems.map((item) => (
              <NavLink
                key={item.to}
                to={item.to}
                end={item.end}
                className={({ isActive }) =>
                  `rounded-md px-3 py-1.5 text-sm transition-colors ${
                    isActive
                      ? "bg-accent font-medium text-accent-foreground"
                      : "text-muted-foreground hover:bg-secondary hover:text-foreground"
                  }`
                }
              >
                {item.label}
              </NavLink>
            ))}
            <a
              href="https://github.com/dev-amjad-shaikh/rusty"
              target="_blank"
              rel="noreferrer"
              aria-label="GitHub repository"
              className="ml-1 rounded-md p-2 text-muted-foreground transition-colors hover:bg-secondary hover:text-foreground"
            >
              <Github size={17} />
            </a>
          </nav>
        </div>
      </header>

      <main className="flex-1">
        <Outlet />
      </main>

      <footer className="border-t bg-secondary/40">
        <div className="mx-auto grid max-w-6xl gap-8 px-4 py-10 sm:grid-cols-3 sm:px-6">
          <div>
            <div className="flex items-center gap-2">
              <span className="flex h-6 w-6 items-center justify-center rounded-md bg-primary text-primary-foreground">
                <Flame size={13} strokeWidth={2.4} />
              </span>
              <span className="font-display font-semibold">rusty</span>
            </div>
            <p className="mt-3 max-w-xs text-sm leading-relaxed text-muted-foreground">
              The durable agent runtime built in Rust. Graphs, super-steps, and
              a checkpoint at every step boundary — deployed as one static
              binary.
            </p>
          </div>
          <div>
            <h4 className="text-sm font-semibold">Site</h4>
            <ul className="mt-3 space-y-2 text-sm text-muted-foreground">
              <li>
                <Link to="/" className="hover:text-foreground">
                  Overview
                </Link>
              </li>
              <li>
                <Link to="/learn" className="hover:text-foreground">
                  Learn
                </Link>
              </li>
              <li>
                <Link to="/playground" className="hover:text-foreground">
                  Playground
                </Link>
              </li>
            </ul>
          </div>
          <div>
            <h4 className="text-sm font-semibold">Project</h4>
            <ul className="mt-3 space-y-2 text-sm text-muted-foreground">
              <li>
                <a
                  href="https://github.com/dev-amjad-shaikh/rusty"
                  target="_blank"
                  rel="noreferrer"
                  className="hover:text-foreground"
                >
                  GitHub repository
                </a>
              </li>
              <li>License: MIT OR Apache-2.0</li>
              <li>MSRV: Rust 1.86</li>
            </ul>
          </div>
        </div>
        <div className="border-t py-4 text-center font-code text-xs text-muted-foreground">
          aboutrusty.com — v0.x under active development
        </div>
      </footer>
    </div>
  );
}
