import { type RouteConfig, index, layout, route } from "@react-router/dev/routes";

/** Pages whose feature lands in a later milestone share one placeholder module. */
const comingSoon = (path: string) => route(path, "routes/app/coming-soon.tsx", { id: `coming-soon:${path}` });

export default [
  route("login", "routes/login.tsx"),
  route("auth/verify", "routes/auth-verify.tsx"),

  // Everything below requires a signed-in user (see clientMiddleware in the layout).
  layout("routes/app/layout.tsx", [
    index("routes/app/home.tsx"),
    route("settings", "routes/app/settings.tsx"),
    route("videos", "routes/app/videos.tsx"),
    route("watch/:id", "routes/app/watch.tsx"),
    route("trash", "routes/app/trash.tsx"),
    ...["photos", "map", "history", "playlists", "albums", "liked", "shared", "search"].map(comingSoon),
  ]),

  route("*", "routes/not-found.tsx"),
] satisfies RouteConfig;
