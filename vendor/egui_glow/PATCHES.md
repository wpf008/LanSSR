# LanBridge 的 egui_glow 本地补丁

本目录来自 `egui_glow 0.33.3`，上游项目为
[emilk/egui，0.33.3 标签](https://github.com/emilk/egui/tree/0.33.3/crates/egui_glow)。
LanBridge 选择其 `MIT OR Apache-2.0` 双许可证中的 MIT 选项。
`LICENSE-MIT` 保留上游完整原文，来源为同一标签的根目录许可证；
Git blob 为 `673ea5f0505d6fa65c6a2b6a753c93b8e19d11b9`，SHA256 为
`95ca92f5f8ea5231f1580b3a2a799e8260af3114b900e1def5355a7f44bcf60c`。

## 变更原因

Windows 7 未安装显卡厂商驱动时，系统 `opengl32.dll` 可建立 OpenGL 1.1
窗口，而上游画家要求 OpenGL 2.0 以上，因此在窗口初始化期间报错。
本地补丁让桌面 OpenGL 1.x 使用固定管线画家；支持 OpenGL 2.0 以上和
OpenGL ES 的上下文继续使用上游的着色器路径。

## 兼容渲染

- `src/legacy.rs` 从操作系统 OpenGL 库加载核心 1.1 接口；Windows 仅使用
  系统 `opengl32.dll`，没有额外 ANGLE、Mesa 或其他图形 DLL。
- 使用正交投影、纹理与顶点颜色相乘、预乘透明度混合和剪裁矩形。顶点及
  索引保留在 CPU 数组中，直接调用 `glDrawElements` 并传入完整原生指针，
  避免将 64 位 CPU 指针误当作 32 位缓冲偏移。
- 不创建着色器、VAO、VBO 或 FBO，不调用活动纹理、分离混合或生成
  mipmap 等较新接口。
- 管理纹理以 2 的幂尺寸作为内部区域，外加 OpenGL 1.1 的一像素纹理
  边框；复制边缘像素并缩放 UV。支持 OpenGL 1.2 或边缘钳制扩展的
  驱动使用 `CLAMP_TO_EDGE`；纯 OpenGL 1.1 使用 `GL_CLAMP` 及复制的
  边框。这使非 2 的幂图像、线性字体采样及 egui 的 `WHITE_UV=(0,0)`
  保持正确。纹理内部区域可使用完整的
  `GL_MAX_TEXTURE_SIZE`，边框不从该上限扣除。
- 局部纹理更新只上传受影响的区域；边缘变化同步更新填充及边框，保留
  其他像素。上传后调用核心 1.1 的 `glGetTexLevelParameteriv` 查询
  实际 `GL_TEXTURE_BORDER`；部分兼容驱动会去掉请求的边框，此时后续
  更新裁剪为实际内部区域并使用非负坐标，避免非法的负偏移。
  最近邻采样保持传输棋盘格和水印像素的原始颜色。
- 去掉边框且未提供边缘钳制能力的兼容驱动额外对线性纹理的 UV 做
  半像素钳制，避免 `WHITE_UV` 面板采样到默认透明边框。此特殊分支
  会微调线性整图映射的外边缘及触边字形的插值，字体像素可能与正常
  边缘钳制路径不同；最近邻传输纹理、真实带边框的核心 1.1 驱动和
  现代着色器路径不采用该调整。
- LanSender 依据上下文的纹理上限限制画面格子数量；减少容量而不重采样
  水印，避免软件 OpenGL 的较小纹理上限导致初始化或传输画面上传失败。

## 范围与验证

兼容路径支持本应用使用的最近邻、线性采样及边缘钳制。
任意非 2 的幂重复纹理、镜像重复和 mipmap 并未模拟：不适用的重复模式
降级为边缘钳制，mipmap 降级为对应的基础采样模式。

`legacy-test` 特性仅供验证：在支持固定管线的桌面上下文强制进入兼容
画家。发布构建不启用该特性；正常构建依照实际 OpenGL 版本选择。
纹理算法单元测试和真实软件 OpenGL 像素验证记录见
`../../docs/validation.md` 与 `../../dist/validation-2.0.14/`。
Linux 软件 OpenGL 测试不等于 Windows 7 实机启动或吞吐量验证。
