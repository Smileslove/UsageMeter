# Git 工作树项目归属设计

## 问题与目标

本地 token 来源是 agent transcript/数据库，而非项目源文件。Claude Code、Codex 等扫描器已从日志中取得 cwd 和 usage，但项目聚合以原始 cwd 为键；目录名只是展示名称。主目录、工作树和仓库子目录因此分裂成多个项目。项目详情又按名称筛选会话，造成同名仓库混入。

目标是将同一个本地 Git 仓库的工作树用量汇总，同时保留会话的实际 cwd；不改变请求去重、token/费用计算或会话 ID。独立 clone、子模块分别统计，不按 remote URL、分支名或目录名推测关联。

## 身份与解析

从 cwd 向上查找最近的 .git。普通目录直接使用 .git；gitfile 按 gitdir 解析绝对/相对路径；linked worktree 再按 commondir 取得 canonical common directory。最近的 .git 是边界，避免越过子模块或损坏的仓库标记归入父仓库。

common directory 用来确认共享仓库关系。主工作区从实际 .git 标记取得根路径；linked worktree 通过 Git 官方 [`worktree list --porcelain -z`](https://git-scm.com/docs/git-worktree) 查询主工作区路径，规范化后作为现有项目 API 的 projectKey/projectPath，名称取主目录末段。裸仓库使用列表中的裸仓库路径。不能假定 common directory 的父目录是主项目（separate-git-dir 并不满足此规则）。Git 子进程使用参数数组、清除会影响仓库选择的 Git 环境变量、隐藏 Windows 控制台、设定超时和输出上限。

## 持久化与缓存

新增两层持久化：local_project_paths(cwd PRIMARY KEY, common_dir, project_path, project_name) 只保存当前目录的发现结果；local_session_projects(session_id PRIMARY KEY, common_dir, project_path, project_name) 保存历史会话已经确认的仓库身份。读取、再次解析 transcript 和同步导出都使用会话绑定，避免目录被其他仓库复用后把旧 token 转给新项目。重建用量缓存保留关联，以便已删除工作树的旧日志恢复归属。

同一轮扫描按 common directory 复用主目录查询结果；先发现所有有效主工作区，再解析工作树，以便 separate-git-dir 主目录移动后更新同仓库归属。缓存及旧主路径必须重新核对真实 Git common directory 和主工作区边界，不能仅凭目录存在复用。当前仓库可识别但主目录不可识别时，只允许同仓库、有效的旧关联回退。目录消失可保留其发现结果，目录存在但不是 Git 仓库则撤销发现结果；两种情况都不会撤销已确认的历史会话绑定。

扫描完成后独立于 transcript 指纹刷新归属，无须重解析 token。未绑定的会话从当前目录发现仓库；已绑定会话只追随相同 common directory 的有效主路径更新。更新 session/request/source 的项目键和同步 outbox，原始 cwd 保持不变。历史缓存同时收集本地请求日期和该会话的统一物化事实日期，覆盖 proxy-only 请求。已淘汰逐请求事实但保留汇总的日期无法再反查会话，发生归属更新时保守地让这些日期汇总失效；其他完整缓存按受影响日期失效。

## 聚合与展示

SessionMeta 新增可选 project_path；会话和请求事实优先使用该字段，缺失时使用原 cwd，最后使用名称/unknown。SessionStats 新增可选 projectKey，项目详情按相同键匹配会话。旧接口数据缺少 projectKey 时按 cwd 或 name::名称回退，不再仅凭名称匹配两个有不同路径的仓库。

现有同步格式已有 project_key 和 project_name，继续使用这些字段携带规范归属；远端不对发送设备的路径执行本地 Git 解析。绝对路径仅定义单设备上的本地仓库身份，不承诺跨设备、独立 clone 自动合并。

## 升级与失败行为

数据库 v36 创建目录映射表，v37 增加会话绑定表并从已有映射迁移已知归属，清除旧物化缓存；不删除历史 token。如果升级前历史身份已被错误覆盖、原始目录已不可访问，不能仅凭旧映射恢复丢失的身份。后台同步完成后自动回填；首次首屏无同步读取可以短暂显示旧统计，沿用现有后台刷新机制。新日志仍按现有解析器扫描。没有 cwd、权限不足、WSL/容器内路径不能在本机访问、首次发现时工作树已删除，均沿用原归属。

对于 separate-git-dir 主工作区可直接识别；其 linked worktree 若 Git 仅返回元数据目录且没有已知主工作区映射，则保留原归属，待主工作区关联建立后再归并。Git 缺失时仍能识别普通主工作区和复用已有映射，但新发现的 linked worktree 使用原归属。

本版本保持会话级归属：会话中途切换工作目录、父会话包含多个仓库的子 agent，仍遵循现有会话 cwd。请求级多仓库归属需要额外的日志格式支持，留待独立需求。

## 验证

真实临时 Git 仓库：主目录、不同名字的工作树、工作树子目录、relative gitfile、独立同名仓库、separate-git-dir 和裸仓库。数据库：不变日志回填、删除工作树后保留、再次解析后复用、映射变化缓存失效、同步导出/outbox 归属一致、token 数值和请求数量不变。前端：工作树按 projectKey 归入主项目，同名仓库分离，旧字段回退。执行 Rust 测试/格式与静态检查及相关前端测试。

## 实现落点与验证结果

| 环节 | 实现位置 | 行为 |
| --- | --- | --- |
| Git 关系解析 | session/project.rs | gitfile、commondir、子仓库边界、原生路径规范化、主目录查询 |
| 关联回填 | local_usage/database/project.rs | 按会话保存归属、按仓库刷新主目录、同步 outbox 更新、local/proxy 历史缓存失效 |
| 读取和入库 | database/queries.rs、scanner_sync.rs | 原始 cwd 保留、读取规范项目路径、已知映射在入库时复用 |
| 请求及项目聚合 | unified_usage/types.rs、derived_support.rs | local/proxy/merged 请求和元数据会话使用一致项目路径 |
| 会话筛选 | DesktopProjects.vue、proxy/types.rs | 按项目键归属，同名仓库隔离 |

验证在 macOS 本地运行；Windows 路径和控制台分支尚未在 Windows 实机验证。修复后 Rust 全量 935 个测试通过，另补充的 1 个元数据会话缓存回归测试单独通过；前端全量 165 个测试通过。前端生产构建及 Rust Clippy（warnings 视为错误）通过。测试覆盖实际临时仓库与临时 SQLite 数据库，未以当前用户的用量数据库作为写入验证对象。

## 审查问题与修复

| 问题 | 分析结论 | 修复 |
| --- | --- | --- |
| cwd 从仓库 A 复用于 B 后旧会话误归 B | 目录映射代表当前状态，不能代表历史身份，确实存在 | 增加会话级绑定，读取、重扫和回填均保持原仓库；迁移保存已有身份 |
| proxy-only 历史缓存未失效 | 本地请求日期覆盖不到代理请求，确实存在 | 查统一物化事实的会话日期；无明细的已淘汰汇总保守失效 |
| 旧主目录复用后仍被缓存信任 | is_dir 不能证明仓库身份，确实存在 | 验证真实 common directory 和主目录边界；有效新主目录优先，失效缓存剔除 |
| 已识别新仓库却回退到旧仓库 | 未限定 previous 的 common directory，确实存在 | 限制旧关联必须属于当前仓库且有效；失败时保持新会话原归属 |

先增加回归测试验证四个问题均能复现，再实施修复。补充覆盖 v36 升级、目录复用后的 transcript 重扫、完整及已淘汰的 proxy-only 缓存，以及同一 resolver 生命周期中的主目录复用。

修复后独立子代理只读复审确认四项根因均已处理，未发现新的可操作问题。Rust 格式检查、Clippy（`-D warnings`）、前端生产构建及 `git diff --check` 均通过。测试检查持久化缓存失效与重建后的归属字段，尚未驱动完整 GUI/后台同步端到端流程。
