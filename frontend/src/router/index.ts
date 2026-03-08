import type { RouteLocation, RouteRecordRaw } from "vue-router";
import { createRouter, createWebHistory } from "vue-router";
import Login from "@/views/Login.vue";
import Layout from "@/views/Layout.vue";
import Files from "@/views/Files.vue";
import Share from "@/views/Share.vue";
import Users from "@/views/settings/Users.vue";
import User from "@/views/settings/User.vue";
import Settings from "@/views/Settings.vue";
import GlobalSettings from "@/views/settings/Global.vue";
import ProfileSettings from "@/views/settings/Profile.vue";
import Shares from "@/views/settings/Shares.vue";
import Errors from "@/views/Errors.vue";
import { useAuthStore } from "@/stores/auth";
import { baseURL, liteMode, name } from "@/utils/constants";
import i18n from "@/i18n";
import { recaptcha, loginPage } from "@/utils/constants";
import { login, validateLogin } from "@/utils/auth";

const titles = {
  Login: "sidebar.login",
  Share: "buttons.share",
  Files: "files.files",
  Settings: "sidebar.settings",
  ProfileSettings: "settings.profileSettings",
  Shares: "settings.shareManagement",
  GlobalSettings: "settings.globalSettings",
  Users: "settings.users",
  User: "settings.user",
  Forbidden: "errors.forbidden",
  NotFound: "errors.notFound",
  InternalServerError: "errors.internal",
};

const routes: RouteRecordRaw[] = [
  ...(!liteMode
    ? [
        {
          path: "/login",
          name: "Login",
          component: Login,
        },
        {
          path: "/share",
          component: Layout,
          children: [
            {
              path: ":path*",
              name: "Share",
              component: Share,
            },
          ],
        },
      ]
    : []),
  {
    path: "/files",
    component: Layout,
    meta: {
      requiresAuth: true,
    },
    children: [
      {
        path: ":path*",
        name: "Files",
        component: Files,
      },
    ],
  },
  ...(!liteMode
    ? [
        {
          path: "/settings",
          component: Layout,
          meta: {
            requiresAuth: true,
          },
          children: [
            {
              path: "",
              name: "Settings",
              component: Settings,
              redirect: {
                path: "/settings/profile",
              },
              children: [
                {
                  path: "profile",
                  name: "ProfileSettings",
                  component: ProfileSettings,
                },
                {
                  path: "shares",
                  name: "Shares",
                  component: Shares,
                },
                {
                  path: "global",
                  name: "GlobalSettings",
                  component: GlobalSettings,
                  meta: {
                    requiresAdmin: true,
                  },
                },
                {
                  path: "users",
                  name: "Users",
                  component: Users,
                  meta: {
                    requiresAdmin: true,
                  },
                },
                {
                  path: "users/:id",
                  name: "User",
                  component: User,
                  meta: {
                    requiresAdmin: true,
                  },
                },
              ],
            },
          ],
        },
      ]
    : []),
  {
    path: "/403",
    name: "Forbidden",
    component: Errors,
    props: {
      errorCode: 403,
      showHeader: true,
    },
  },
  {
    path: "/404",
    name: "NotFound",
    component: Errors,
    props: {
      errorCode: 404,
      showHeader: true,
    },
  },
  {
    path: "/500",
    name: "InternalServerError",
    component: Errors,
    props: {
      errorCode: 500,
      showHeader: true,
    },
  },
  {
    path: "/:catchAll(.*)*",
    redirect: (to: RouteLocation) => {
      const catchAll = to.params.catchAll;
      const parts = Array.isArray(catchAll)
        ? catchAll
        : typeof catchAll === "string"
          ? [catchAll]
          : [];
      return `/files/${parts.join("/")}`;
    },
  },
];

async function initAuth() {
  if (liteMode) {
    const authStore = useAuthStore();
    authStore.setUser({
      id: 1,
      username: "lite",
      password: "",
      scope: "/",
      locale: navigator.language || "en",
      lockPassword: false,
      hideDotfiles: false,
      singleClick: false,
      redirectAfterCopyMove: false,
      dateFormat: false,
      viewMode: "list",
      sorting: { by: "name", asc: true },
      aceEditorTheme: "chrome",
      commands: [],
      rules: [],
      perm: {
        admin: false,
        copy: true,
        create: true,
        delete: true,
        download: true,
        execute: false,
        modify: true,
        move: true,
        rename: true,
        share: false,
        shell: false,
        upload: true,
      } as IUser["perm"],
    });
    return;
  }

  if (loginPage) {
    await validateLogin();
  } else {
    await login("", "", "");
  }

  if (recaptcha) {
    await new Promise<void>((resolve) => {
      const check = () => {
        if (typeof window.grecaptcha === "undefined") {
          setTimeout(check, 100);
        } else {
          resolve();
        }
      };

      check();
    });
  }
}

const router = createRouter({
  history: createWebHistory(baseURL),
  routes,
});

router.beforeResolve(async (to, from, next) => {
  const title = i18n.global.t(titles[to.name as keyof typeof titles]);
  document.title = title + " - " + name;

  const authStore = useAuthStore();

  // this will only be null on first route
  if (from.name == null) {
    try {
      await initAuth();
    } catch (error) {
      console.error(error);
    }
  }

  if (!liteMode && to.path.endsWith("/login") && authStore.isLoggedIn) {
    next({ path: "/files/" });
    return;
  }

  if (to.matched.some((record) => record.meta.requiresAuth)) {
    if (!authStore.isLoggedIn) {
      next({
        path: "/login",
        query: { redirect: to.fullPath },
      });

      return;
    }

    if (to.matched.some((record) => record.meta.requiresAdmin)) {
      if (authStore.user === null || !authStore.user.perm.admin) {
        next({ path: "/403" });
        return;
      }
    }
  }

  next();
});

export { router, router as default };
