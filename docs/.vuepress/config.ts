import { defineUserConfig } from "vuepress";
import { hopeTheme } from "vuepress-theme-hope";

export default defineUserConfig({
  base: process.env.BASE || "/",
  locales: {
    "/": {
      lang: "en-US",
      title: "Oak Video Editor",
      description:
        "Open-source, non-linear video editor focused on speed and clarity.",
    },
    "/zh/": {
      lang: "zh-CN",
      title: "Oak 视频编辑器",
      description: "面向创作者的开源非线性剪辑软件。",
    },
  },
  theme: hopeTheme({
    logo: "/images/oak-icon.png",
    locales: {
      "/": {
        selectLanguageName: "English",
        navbar: [
          { text: "Home", link: "/" },
          { text: "Build", link: "/build.html" },
          { text: "Project Files", link: "/project-file-reference.html" },
          { text: "Test Plan", link: "/test-plan.html" },
        ],
        sidebar: [
          {
            text: "Documentation",
            children: [
              "/build.md",
              "/project-file-reference.md",
              "/test-plan.md",
            ],
          },
        ],
      },
      "/zh/": {
        selectLanguageName: "简体中文",
        navbar: [
          { text: "首页", link: "/zh/" },
          { text: "构建", link: "/zh/build.html" },
          { text: "工程文件", link: "/zh/project-file-reference.html" },
          { text: "测试计划", link: "/zh/test-plan.html" },
        ],
        sidebar: [
          {
            text: "文档",
            children: [
              "/zh/build.md",
              "/zh/project-file-reference.md",
              "/zh/test-plan.md",
              "/zh/structure.md",
            ],
          },
        ],
      },
    },
  }),
});
