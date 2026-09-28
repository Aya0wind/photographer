# 阶段 F 前置条件评估：网盘交付（百度网盘优先）

> 调研时点 **2026-09-28**。纯调研决策文档，不含代码改动。依据 [implementation-plan.md §10/§12](../implementation-plan.md) 与 [2026-09-27-followup-roadmap.md §2.F](./2026-09-27-followup-roadmap.md)（"官方开放平台接口核实为前置条件"）。所有外部结论均注来源与信息年份；查不到的写"未核实"。

## 0. 结论先行

| 问题 | 结论 |
|---|---|
| 百度网盘**文件上传** API 对个人开发者可行吗 | **可行**（个人实名认证 + 自建应用 + `basic,netdisk` scope，precreate→分片→create 三段式上传，官方声明"仅对异常行为限制，不影响正常使用"） |
| 百度网盘**创建分享链接** API 对个人开发者可行吗 | **不可行**。官方接入指南明确：文件分享服务"属于**企业开发者专属权益**"，且是"**付费开放能力**，购买服务后才可正式使用"（2026 年现状） |
| 首接哪个适配器 | **通用"本地交付目录"底座 + 百度网盘"API 上传 + 手动回填分享链接"半自动模式**。全自动化分享链接在百度侧对个人已关闭，不应作为阶段 F 的验收门槛 |
| 回退方案（本地交付目录 + 交付记录）还值得建吗 | **值得，且应升格为第一优先**。它是所有非 API 渠道（官方客户端手动上传、U 盘、NAS 共享）的公共底座，`delivery_job`/`delivery_link` 表按 provider 无关设计后，百度适配器只是其中一个 provider |

**一句话推荐路线**：先做 provider 无关的交付模型（本地交付目录 + manifest 清单 + 一键打开 + `delivery_job/delivery_item/delivery_link` 三表，迁移 0023），再做百度适配器 v1 = 官方 OAuth（用户自建 AppKey）+ 三段式上传 + 上传完成后引导用户在官方客户端创建分享、把链接**手动贴回**应用入库；百度"API 直建分享链接"降级为远期可选（除非未来政策变化或项目走企业认证）。坚果云 WebDAV 作为第二个适配器候选（官方明确允许第三方客户端，但免费档流量 1GB/月不够照片交付，且 WebDAV 协议本身无法创建分享链接）。

## 1. 百度网盘开放平台现状（逐项调研）

官方入口：[pan.baidu.com/union](https://pan.baidu.com/union)，文档中心 [pan.baidu.com/union/doc/](https://pan.baidu.com/union/doc/)。

### 1.1 OAuth 2.0 授权流程 —— 可用

来源：[接入授权-授权介绍](https://pan.baidu.com/union/doc/使用入门/接入授权/授权介绍/)（官方文档，抓取于 2026-09-28）

- 三种模式：**授权码模式**（有 Server 端，Code 换 Token）、**简化模式**（无 Server 端直发 Token，**不支持刷新**）、**设备码模式**（弱输入设备）。桌面应用（Tauri）适用授权码模式（回调 `oob` 或本地回环地址，社区通行做法，见 §2.3 案例）。
- **Access Token 有效期 30 天，过期可刷新，刷新后仍是 30 天**。
- **refresh_token 一次性**：使用后失效，下次刷新必须用上一次响应里的新 refresh_token（滚动轮换）。刷新请求失败时旧 refresh_token 失效，需重新走授权。→ 落库设计必须"原子更新两个 token"，且令牌只能存系统凭据库（与路线图 §10 一致）。
- 官方提醒"按需刷新，不要不停刷新"。

### 1.2 文件上传（precreate + superfile2 分片 + create） —— 可用

来源：[上传-能力说明](https://pan.baidu.com/union/doc/基础网盘服务/上传/能力说明/)（官方文档，2026-09-28 抓取；子页：预上传/分片上传/创建文件/单步上传/获取上传域名）

- 三阶段：**预上传（precreate）→ 分片上传（superfile2）→ 创建文件（create）**，依次依赖；官方提供 Go 版上传 SDK 与调试工具。
- 单文件上限按会员分级：**普通用户 4GB / 会员 10GB / 超级会员 20GB**；分片数 ≤1024。RAW 单张几十 MB 远低于门槛。
- **目录沙箱**：第三方应用只能写入 `/apps/{应用名}`（用户网盘内展示为 `/我的应用数据/{应用名}`）。文件在该目录下对用户可见，用户可自行在官方客户端移动或分享。
- 频控："开放平台仅对异常行为进行相应限制，不会影响正常使用"——上传类接口**无量化频次限制**（官方原文）。

### 1.3 创建分享链接 API 的权限门槛 —— 个人不可行（决定性结论）

来源：[文件分享服务（新）-接入指南](https://pan.baidu.com/union/doc/基础网盘服务/文件分享服务新/接入指南/) 与 [创建分享链接](https://pan.baidu.com/union/doc/基础网盘服务/文件分享服务新/创建分享链接/)（官方文档，2026-09-28 抓取）

- 接入指南原文（逐字）："**文件分享服务属于企业开发者专属权益，需要先完成企业开发者认证**"；"**文件分享服务为付费开放能力，购买服务后才可正式使用**"。
- 购买渠道：添加客户经理企业微信，或邮件 `ext_mars-union@baidu.com`（1~3 个工作日联系）。**价格未公开（未核实）**。
- 接口本体：`POST /apaas/1.0/share/set`（OAuth 2.0 鉴权），参数含 `fsid_list`、`period`（天）、`pwd`（4 位提取码，必填）、`ticket`（可附送极速流量包权益）。注意这是"文件分享服务新"的新端点，旧文档流传的 `/rest/2.0/xpan/share?method=set` 属旧版口径。
- 附带限制：与百度网盘企业版不互通；接口有频次限制（数值未公开）。
- **含义**：个人摄影师（本项目用户画像）无法通过官方 API 自动创建分享链接。这不是审核问题，是**资质 + 付费双重门槛**。

### 1.4 调用频率限制与会员限速

来源：[权限与配额](https://pan.baidu.com/union/doc/使用入门/权限与配额/)（官方文档，2026-09-28 抓取）

| 维度 | 未通过/未提交上线审核的应用 | 已通过上线审核的应用 |
|---|---|---|
| 接口访问权 | 按接口单独说明 | 白名单制，按需分配 |
| 频率控制 | **10 次/小时** | 按需分配（需逐接口申请扩容） |
| 用户数量上限 | **10 个** | 不限制 |

- 超限错误码：20011（用户数）/ 20012（频率、次数）/ 20013（无权限）。
- **关键交叉解读**：[应用上线审核指南](https://pan.baidu.com/union/doc/使用入门/应用上线审核/应用上线审核试运行/)（更新于 2026-08-03）规定"**个人使用**应用创建后直接可用、免上线审核、限 ≤10 人使用"，且补录用途选"个人使用"后应用状态直接变为**已上线**。因此个人使用应用大概率**不落入"未提交审核 = 10 次/小时"档**；但官方未逐字写明"个人使用已上线应用"的频控档位（**未核实，需实测**）。上传类接口另有"仅限制异常行为"的官方兜底（§1.2），对一次交付几十上百张照片的场景够用。
- **会员限速**：官方上传文档只按会员区分单文件大小，未量化传输速度（未核实）。社区实测（AList/rclone 挂载指南，2025-2026）确认**非会员下载被限速**（常见 1~2MB/s 甚至更低）；上传限速无权威量化数据。交付场景主要是上传，速度风险低；但客户**下载**这些分享时受分享方会员等级影响——这也是"半自动模式"里让用户在官方客户端创建分享的原因之一（官方渠道下载体验由百度自己负责）。

## 2. 开发者注册门槛

来源：[实名认证介绍](https://pan.baidu.com/union/doc/使用入门/实名认证/实名认证介绍/)（官方文档，页面标注更新 2022-03-24，2026-09-28 仍在线）、[应用上线审核指南](https://pan.baidu.com/union/doc/使用入门/应用上线审核/应用上线审核试运行/)（更新 2026-08-03）

### 2.1 个人开发者能否注册

**能**（按官方文档）：个人实名认证仅需**身份证号 + 邮箱/手机号**，"可立即审核完成"，账号归属个人，可创建 **1 个应用**（企业认证可建 10 个）。个人与企业都包含基础能力："获取网盘状态信息、获取网盘文件信息、**上传、下载、管理文件**"及视频/智能相册服务；仅企业额外开放智能设备入驻、智能小程序入驻。文件分享服务为文档后加的"企业专属 + 付费"能力（§1.3）。

**时效风险提示（必须记录）**：开源项目 BaiduPan.el 于 2024-07-03 停更，README 原文称"目前百度开放平台已经不允许个人用户接入"（[github.com/lorniu/BaiduPan.el](https://github.com/lorniu/BaiduPan.el)，2024）。但与此后的事实矛盾：2026-08-03 更新的官方审核指南仍保留"个人使用应用免审"规则；2025-2026 年社区教程（如 [CSDN：Python 对接百度网盘 OpenAPI](https://blog.csdn.net/jiangfuofu555/article/details/162038887)）仍在引导个人注册并成功创建桌面应用。两种可能：2024 年的临时收紧已恢复，或注册入口对部分账号风控。**最终以实际注册为准（需实测）**；接入申请页 `pan.baidu.com/union/apply` 需登录百度账号后才能看到表单（本次未登录未能核实）。

### 2.2 个人开发者的具体注册步骤（按官方文档整理，实测待验证）

1. 百度账号登录 [pan.baidu.com/union](https://pan.baidu.com/union)。
2. 控制台发起**实名认证**，选"个人"：填身份证号 + 邮箱/手机号，即时审核。
3. **创建应用**：类型选"软件/桌面应用"，用途选"**个人使用**"（≤10 人，免上线审核，直接可用）；勾选权限 `basic, netdisk`。
4. 配置回调地址：桌面应用可填 `oob`（授权码页面显示复制模式，社区通行）。
5. 获得 **AppKey / SecretKey**（控制台应用详情）。
6. 走授权码模式拿 access_token / refresh_token（30 天/滚动轮换，§1.1）。

注：官方"个人最多 1 个应用"（2022 版文档）；BaiduPan.el 2022 年 README 写"最多 2 个"——口径可能变过，以控制台实际为准（未核实）。

### 2.3 个人开源工具的实际过审/使用案例

- **用户自建 AppKey 模式**（不开源仓库 key、每位用户自己注册）是社区通行合规做法：AList 的 BaiduNetdisk(Open) 驱动、[BaiduPCS-Go（qjfoidnh 维护版）](https://github.com/qjfoidnh/BaiduPCS-Go)、各语言 OpenAPI 教程均引导用户自行到 pan.baidu.com/union 创建应用拿 AppKey/SecretKey。
- **公开案例空白**：未找到个人开源工具以"公开发布使用"用途**通过百度上线审核**的公开案例（未核实）。公开发布审核要求（2026-08-03 指南）：应用名/描述/功能类别、**测试账号 + 操作指引**、申请 API 列表及必要性说明、**演示视频**（展示 OAuth 登录与主流程）、邮箱、付费合作电话；隐私政策 URL 与服务条款 URL 为**选填**。审核团队会实机走查"安装→OAuth→核心功能→卸载"全流程。
- **对本项目的取舍**：Photo Hub 当前定位个人自用 + 开源，应用选"个人使用"即可（免审）；将来若要给其他摄影师公开分发使用，再评估提交上线审核（材料清单已列，演示视频与测试账号是主要工作量），或维持"用户自建 key"模式规避审核。

## 3. 条款红线（与 Apache-2.0 开源分发的关系）

来源：[开放平台简介-使用规范](https://pan.baidu.com/union/doc/)、[开发者服务协议](https://pan.baidu.com/union/protocol/)（术语含义/服务的使用及管理章节，2026-09-28 抓取）

### 3.1 明确的禁止行为（平台简介-使用规范，逐字）

1. "侵犯用户数据隐私，开发者**未经用户授权**、用户不明确知晓的场景中违规下载使用、传播用户网盘数据"——Photo Hub 只在用户 OAuth 授权后操作其本人网盘，合规。
2. "**多人共享开发者账号**、多人共享个人网盘账号及会员权益"——个人使用应用限 10 个授权用户；Photo Hub 单机自用 = 摄影师授权自己的网盘，1 个用户，合规。**红线含义：不能在开源仓库或文档里散布一个公共 AppKey 给所有人共用**。
3. "利用个人网盘账号进行**企业建站、图床、搭建网盘迁移工具**等产品行为"——Photo Hub 是照片管理 + 交付工具，不是网盘迁移/图床产品，不在此列；但发展功能时避免做成"通用网盘搬运器"形态。

### 3.2 商用与竞争限制（服务协议"服务的使用及管理"）

- 不得"以任何形式使用开放平台服务侵犯度友公司的商业利益，包括并不限于**发布非经度友公司许可的商业广告**"；不得"利用接口**开展与度友公司有竞争关系的**业务"；不得"为任何第三方申请接口"。
- Photo Hub 免费、无广告、不做网盘服务 → 不冲突。**用户**用软件交付收费照片是使用场景，不是项目方"利用接口经商"（与路线图 §12 的"使用场景 vs 分发依赖"区分一致）。
- 数据归属条款："'用户数据'的所有权及其他相关权利属于**度友公司**…依法属于用户享有的相关权利除外"——只做透传/展示、不留存第三方用户数据即可。

### 3.3 Apache-2.0 开源仓库分发的合规姿势

- 服务协议未见"禁止开源分发应用"的条款（已核对术语含义、服务的使用及管理两章；**其余章节未逐字核对，标注**）。**[品牌规范](https://pan.baidu.com/union/doc/规范标准/品牌规范/概述/) 单独成册**：公开发布时对"百度网盘"名称/Logo 的使用有要求，本次未逐条核实——文档与 UI 文案中提"百度网盘"时按最小必要引用，不用其 Logo 做应用标识。
- **密钥与令牌保密**：
  - 服务协议未见逐字的 AppKey/SecretKey 保密条款（本次核对范围内，未核实其余章节），但 §3.1 红线 2（不得共享开发者账号）实质等效；
  - [创建分享链接 API 文档](https://pan.baidu.com/union/doc/基础网盘服务/文件分享服务新/创建分享链接/)自身强调 access_token 等是敏感凭证，"不应写入日志或截图"；
  - 上传接口要求 **access_token 放 URL query**（官方规定）→ 日志/遥测必须做 URL 脱敏，与路线图 §10"令牌不进 Git、日志、SQLite 明文"一致。
- **落地结论**：仓库不带任何内置 key；首次使用引导用户按 §2.2 自建应用；AppKey/SecretKey/access_token/refresh_token 全部入 Windows 凭据管理器（如 `keyring` crate），SQLite 只存账号别名与非敏感配置。

## 4. 备选网盘一页概览（2026 年现状）

| 网盘 | 官方开放接口 | 个人可用性 | 分享链接 API | 结论 |
|---|---|---|---|---|
| **百度网盘** | 开放平台（OAuth + 上传/下载/文件管理） | 个人可注册（§2，注册入口状态需实测） | **企业专属 + 付费**（§1.3） | 上传可用、分享 API 不可用 → 半自动模式 |
| **夸克网盘** | **无官方开放平台**。社区全是 Cookie 逆向（quark-auto-save 等），有封号与随时失效风险（社区共识，2026） | — | 无官方接口 | **不接**；路线图 §10"仅使用获准且稳定的官方接口"直接排除 |
| **阿里云盘 (Alipan)** | 开放平台存在（openplatform.alipan.com，OAuth + refresh token；AList "Aliyundrive Open" 驱动在用）。历史开发者协议含个人入驻（身份证 + 应用场景）。**官方站点本次网络不可达，scope 清单与个人可否用分享接口：未核实** | 存疑（未核实） | 疑似不向个人开放（社区口径，未核实） | 备选观察项，不首批接入 |
| **坚果云（WebDAV）** | **官方明确支持第三方客户端**：账户信息 → 安全选项 → 第三方应用管理 → 添加**应用密码**（每应用独立密码）。服务器 `https://dav.jianguoyun.com/dav/`。来源：[坚果云帮助中心 WebDAV](https://help.jianguoyun.com/?p=2064)（2026-09-28 抓取） | **可以，且是最无争议的合规通道** | **WebDAV 协议无法创建分享链接**（协议本身没有该操作） | 通用 WebDAV 适配器可行；分享需用户在坚果云客户端手动创建 |

**坚果云 WebDAV 关键限额**（来源：官方帮助中心，2026-09-28）：

- 免费档流量：**上传 1GB/月、下载 3GB/月**，空间不限（官方博客与多方一致口径，如 [坚果云官方博客](https://blog.jianguoyun.com)、[百度企业网盘 FAQ 引述](https://eyun.baidu.com)）；付费专业版约 199.9 元/年、流量配额更大（精确配额官方定价页 JS 渲染，**未核实**）。
- 请求频率：免费 **600 次/30 分钟**，付费 1500 次/30 分钟；单文件上限默认 **500MB**；目录单次列举 750 个文件（需分页）。
- **对照片交付的判断**：一次婚礼成片动辄 2~10GB，免费档 1GB/月上传**不够**；付费档才可用。适合作为"第二个适配器"服务已订阅用户或轻量交付，不适合免费用户的默认路径。

## 5. 回退方案设计输入：本地交付目录值得建，且应先行

**判断：值得。** 理由：(a) 百度分享 API 对个人不可行是高置信结论，"API 不可用回退本地交付目录"从 Plan B 变成主路径；(b) 本地交付目录是所有渠道（官方客户端手动上传、U 盘、NAS 共享、微信）的公共出口；(c) 交付记录（给了谁、什么版本、何时、链接）的业务价值独立于传输通道存在。

### 5.1 交付记录模型（对齐现有 schema 风格：`INTEGER PRIMARY KEY`、TEXT 时间戳、CHECK 约束、迁移只加不改，下一号为 0023）

现状核对：当前迁移到 `0022_EDIT_EXPORT`（`edit_recipe`/`export_job`），全库无 delivery 表（`src-tauri/src/db/migrations.rs`）。建议三表：

```sql
-- 0023_delivery：provider 无关的交付模型
CREATE TABLE delivery_job (
    id           INTEGER PRIMARY KEY,
    album_id     INTEGER REFERENCES album (id),      -- 可空：也支持跨相册自选集合
    provider     TEXT    NOT NULL CHECK (provider IN ('local', 'baidu', 'webdav')),
    target       TEXT    NOT NULL,                    -- local=目录路径 / baidu=/apps/... 远端目录 / webdav=URL 路径
    selection    TEXT    NOT NULL,                    -- 版本选择快照 JSON：原片/成片/指定派生 + 命名规则
    status       TEXT    NOT NULL CHECK (status IN ('queued','running','done','partial','failed','cancelled')),
    total_files  INTEGER NOT NULL,
    total_bytes  INTEGER NOT NULL,
    fingerprint  TEXT,                                -- 幂等键：(provider,target,selection) 内容指纹，防重复发布
    error        TEXT,
    created_at   TEXT    NOT NULL,
    finished_at  TEXT
);

CREATE TABLE delivery_item (
    job_id     INTEGER NOT NULL REFERENCES delivery_job (id) ON DELETE CASCADE,
    asset_id   INTEGER NOT NULL REFERENCES assets (id) ON DELETE CASCADE,
    rel_path   TEXT    NOT NULL,                      -- 交付包内相对路径（命名模板结果）
    size       INTEGER NOT NULL,
    state      TEXT    NOT NULL CHECK (state IN ('pending','uploading','done','skipped','failed')),
    remote_id  TEXT,                                  -- 百度 fsid / webdav etag 等
    error      TEXT,
    PRIMARY KEY (job_id, asset_id)
);
CREATE INDEX idx_delivery_item_state ON delivery_item (job_id, state);

CREATE TABLE delivery_link (
    id          INTEGER PRIMARY KEY,
    job_id      INTEGER NOT NULL REFERENCES delivery_job (id) ON DELETE CASCADE,
    kind        TEXT    NOT NULL CHECK (kind IN ('local_dir', 'share_url', 'manual_paste')),
    url         TEXT,                                 -- local 模式=目录路径；share/manual=链接
    password    TEXT,                                 -- 提取码（百度 4 位等；本身非机密，可与链接同存）
    expires_at  TEXT,                                 -- 链接有效期（百度 period/天）
    created_at  TEXT    NOT NULL,
    revoked_at  TEXT                                   -- 失效时间；重新生成=插入新行，旧行 revoke
);
```

设计要点：

1. **provider 能力位驱动 UI**：适配器声明能力（`can_upload` / `can_share` / `can_resume`）。百度 v1：`can_upload=true, can_share=false` → 上传完成后 UI 转入"去官方客户端创建分享 → 贴回链接"（`kind='manual_paste'`）；将来若政策变化只需翻能力位，表结构不动。
2. **asset 选择挂版本关系**：`selection`/`delivery_item.asset_id` 复用 `photo_group`/`asset_relation`（0020 后的版本模型）表达"原片 vs 成片 vs 指定派生"，不在 delivery 层重复建模版本。
3. **幂等与重试**：`fingerprint` + `delivery_item` 逐文件 state 支撑"中断恢复、失败重试、重复执行不重传"（对齐 jobs/job_files 既有模式）；百度 precreate 本身支持秒传/断点语义（分片 md5），可作为二阶段优化。
4. **本地模式清单**：交付目录写 `manifest.json`（客户/相册/日期/张数/逐文件大小与哈希/许可说明）+ 应用内"一键打开目录"（资源管理器定位）。链接行 `kind='local_dir'`，同样可检索"已交付"状态（路线图 §3 的"是否已交付"筛选）。
5. **令牌零落库**：AppKey/SecretKey/access_token/refresh_token 全部走系统凭据库；SQLite 只存 provider 账号别名。注意 access_token 在 URL query 的官方要求 → HTTP 客户端日志层统一脱敏。
6. **重新发布语义**：同 fingerprint 再发布时提示"覆盖/新版本/跳过"（§10 要求），落在 `delivery_job` 层比较新旧 job。

### 5.2 百度适配器 v1 的验收边界（对应阶段 D/F 验收条款的修正）

- ✅ 原片/成片分别选择交付、后台上传、进度/失败重试、断点恢复 → API 均支持。
- ✅ 交付记录（远端路径、fsid、时间、张数）→ 落库。
- ⚠️ "成功后创建分享链接"修正为：**上传完成 → 引导打开官方客户端/网页（定位到 /我的应用数据/PhotoHub 对应目录）→ 用户创建分享 → 链接+提取码+有效期手动贴回入库**。
- ✅ "API 不可用时仍能完成本地交付目录导出" → 即 §5.1 本地模式，作为所有 provider 失败时的兜底。

## 6. 风险清单与复查机制

| 风险 | 等级 | 缓解 |
|---|---|---|
| 个人注册入口实际关闭（BaiduPan.el 2024 口径 vs 官方 2026 文档矛盾） | 中 | 接入前用真实账号实测注册；若关闭，百度适配器整体降级为"本地交付 + 官方客户端手动上传"指引模式 |
| 个人使用应用频控档位不明（10 次/小时是否适用） | 中 | 上线前用真实 key 压测一次完整相册上传；上传类接口有"仅限异常行为"官方兜底 |
| 免审"个人使用"应用被平台事后收紧/清退 | 低-中 | 适配器做成可拔插 provider；交付记录不依赖单一 provider 存续 |
| 服务协议其余章节、品牌规范未逐字核对 | 低 | 公开发布前补一轮逐条核对（纳入 §12 许可审计清单"网盘 API"行） |
| 非会员客户下载分享限速 | 中 | 属百度侧体验，交付说明中提示；或建议用户开通会员/改用本地交付 |
| 坚果云免费流量不足以交付 | 确定 | 坚果云仅作为付费用户可选项，不设为默认 |
| 百度政策变化（付费/权限/端点） | 常态 | 路线图 §12 已要求"定期重新验证"；在适配器内做端点/版本探测与明确错误上报 |

## 7. 本次调研来源汇总

官方（均于 2026-09-28 抓取，页面自带更新时间已标注）：

- 平台与文档首页：https://pan.baidu.com/union/doc/
- 权限与配额：https://pan.baidu.com/union/doc/使用入门/权限与配额/
- 应用上线审核指南（2026-08-03 更新）：https://pan.baidu.com/union/doc/使用入门/应用上线审核/应用上线审核试运行/
- 实名认证介绍（页面标注 2022-03-24）：https://pan.baidu.com/union/doc/使用入门/实名认证/实名认证介绍/
- 接入授权-授权介绍：https://pan.baidu.com/union/doc/使用入门/接入授权/授权介绍/
- 上传-能力说明：https://pan.baidu.com/union/doc/基础网盘服务/上传/能力说明/
- 文件分享服务（新）-接入指南：https://pan.baidu.com/union/doc/基础网盘服务/文件分享服务新/接入指南/
- 文件分享服务（新）-创建分享链接：https://pan.baidu.com/union/doc/基础网盘服务/文件分享服务新/创建分享链接/
- 开发者服务协议：https://pan.baidu.com/union/protocol/ （术语含义、服务的使用及管理两章）
- 品牌规范（未逐条核实）：https://pan.baidu.com/union/doc/规范标准/品牌规范/概述/
- 坚果云帮助中心-WebDAV：https://help.jianguoyun.com/?p=2064 、https://help.jianguoyun.com/?tag=webdav

社区/第三方（年份已标注）：

- BaiduPan.el 停更声明（2024-07-03）："百度开放平台已经不允许个人用户接入"：https://github.com/lorniu/BaiduPan.el
- BaiduPCS-Go（用户自建 key 生态）：https://github.com/qjfoidnh/BaiduPCS-Go
- Python 对接百度网盘 OpenAPI 教程（2025，个人注册桌面应用 + basic/netdisk + oob 回调）：https://blog.csdn.net/jiangfuofu555/article/details/162038887
- AList + rclone 挂载百度网盘与非会员限速实测（2025-2026，CSDN/腾讯云社区多篇）
- 夸克网盘无官方开放接口、Cookie 逆向生态：https://github.com/Cp0204/quark-auto-save 及社区文章（2026）
- 阿里云盘开放平台（官方站点本次不可达，历史协议与 AList 驱动佐证）：https://github.com/alist-org/alist （Aliyundrive Open 驱动）、开放平台报名页存档
- 坚果云免费档 1GB/3GB 流量：坚果云官方博客 https://blog.jianguoyun.com 及多方一致口径（2026-04）

**未核实项一览**（不要当作事实引用）：文件分享服务价格；个人使用应用的精确频控档位；`union/apply` 当前提交流程表单；个人开源工具通过"公开发布"审核的案例；阿里云盘开放平台 2026 年 scope 与个人分享权限；坚果云专业版精确流量；服务协议未核对章节；品牌规范条文；上传速度是否受会员等级影响。
