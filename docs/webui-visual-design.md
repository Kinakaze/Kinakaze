# WebUI 视觉方向

本轮采用蓝白 ACG 启动台方向：原创角色占独立侧区，操作区保留明确的表格和真实运行数据。用斜切边角、细线与少量珊瑚色细节建立识别；删除宣传口号、无关英文装饰和大面积绿色卡片。插画可通过顶栏按钮收起，设置保存在当前浏览器地址下。

视觉调研参考：

- [Blue Archive 官方站](https://bluearchive.jp/)：参考清透蓝白的层次与角色画面的留白。
- [Collapse Launcher](https://github.com/CollapseLauncher/Collapse)：参考角色背景与功能区域的分工。

以上项目仅用于视觉调研，未将其角色、图标、字体或界面素材复制进项目。

## 原创插画

- 位置：[character.png](../apps/init/src/web/character.png)
- 生成方式：imagegen 技能的内置 `image_gen` 工具（未使用 CLI）。
- 原图直接纳入仓库，实际展示裁切由 CSS 响应式布局控制。
- 素材用途：本地会话侧区装饰，不承载状态、按钮或其他必要信息。

最终提示词：

```text
Use case: stylized-concept. Asset type: original portrait illustration for the left visual rail of an anime-styled desktop software launcher named Kinakaze, not a UI mockup. Create one original young adult female wind courier, age about 22, chest-up portrait, short silver-white bob hair with cool slate-blue shaded underside, clear blue eyes, a small cyan triangular hair clip, calm understated smile. Ivory oversized light technical jacket with navy seams and one small coral zipper pull, fully clothed with high collar, holding a slim closed navy notebook at her chest. Medium shot, character centered in lower two thirds with head fully visible and airy empty upper quarter. Background: very pale cyan summer sky, a few flat white clouds and simple distant rooftop railing, sparse graphic shapes. Authentic clean Japanese 2D anime production illustration, precise fine dark-blue lineart, restrained two-tone cel shading, flat colors, expressive hand-drawn face, minimal material rendering. Portrait 1024x1536. This is an illustration asset, not a website, no interface panels. No letters, words, logos, watermarks, symbols from existing franchises, weapons, sexualization, photorealism, 3D, cinematic bloom, lens flares, sparkles or elaborate gradients.
```
