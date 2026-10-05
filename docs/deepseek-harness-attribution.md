# DeepSeek Harness 请求来源自动归因

## 目标与证据边界

DeepSeek Harness 会话中的 `request/header.data.header.config.provider` 是请求时的路由标识；模型名称不能证明 API 来源。当前会话支持同一会话切换 Provider，压缩/继承日志的请求去重与 seed 排除仍沿用 reader。

`deepseek-account` 标识内置账号 Provider，但账号插件支持自定义平台和推理地址。因此历史请求归到独立的 **DeepSeek Harness 账号** 来源，归因方式为 `provider_reported`；不称为官方 OAuth，不填入推测的 API 地址，不表示余额或供应商账单。这个分组表达日志记录的认证 Provider，不保证网络目的地。API Key Provider 只有在路由配置证据足够时才归到已有 API 来源。

## 数据流

1. reader 随每次 request/header 更新 Provider，并在 assistant usage 事实中携带 `{scope, providerId, requestStartedAtMs}`。scope 是会话根目录的既有 hash，避免不同 DSH_HOME 的同名 Provider 串线。Provider 仅接受长度受限的 ASCII 标识符；不读取 prompt、响应或日志中的凭据。
2. `local_request_facts.provider_evidence` 保存这一非敏感 JSON 证据。扫描器事务写入，统一层按 canonical request key 批量加载；前端不做归因计算。
3. 手动覆盖首先执行，包括明确标为未归因。账号 Provider 使用稳定伪来源；API Key Provider 按 step/start 或 retry-started 的毫秒发起时间（同一步的 request/header 更新可推进该时间）查询对应 scope/Provider 的配置观察时间段，避免响应完成前切换配置造成错归因；统计时间仍使用原事实时间。没有请求发起时间的旧格式不推断 API Key 来源；request/header 可能被多次请求复用，不能将它的旧时间当作每个请求的发起时间。代理事实不被本地推断覆盖。
4. API 来源匹配复用现有 API Key 前缀 + 完整 base URL 规则；新来源通过 update_settings_internal 原子局部更新注册，避免完整快照的实体保护将其丢弃，或覆盖并发修改；数据库仅保存摘要、已脱敏来源标识和合法 URL，不保存密钥或 credential ref。

## API Key 配置适配

独立适配器只理解已核对的 Cordis 配置：`@deepseek-ai/dsh-llm-deepseek-api-key`（路由 deepseek-official）和 `@deepseek-ai/dsh-llm-pi-ai` 的 providers 字典。读取会话目录同级的 profiles/*/cordis.yml、cordis.patch.yml，以及 home/cordis.patch.yml；按 profile 后 global 顺序应用补丁；config 按 Cordis 语义整体替换，name 不匹配的补丁忽略，disabled 仅接受布尔值。只对普通根条目或 insert 建立的可确认路由推断；依赖未读取外部 bundle 才存在的命名补丁保持未归因。只有显式 baseURL 和可读 API Key 才产生观察；不猜测第三方插件、内置模型目录的默认地址或无法解析的运行时表达式。自定义 headers（包括模型级 headers）可能覆盖认证，暂不推断这类路由。

密钥解析只使用 UsageMeter 可见的环境变量或 version=1 的 .credentials.yaml refs；不执行插件、不启动 Harness、不读取项目 .env，也不使用账号 grant。凭据文件在 POSIX 下必须只允许所有者读写。文件大小、配置条目数量、Provider/URL 长度受限，解析错误使用固定代码，不能附带原文。

日志未记录 profile。多个 profile 中同名 Provider 的完整配置必须一致；缺失/冲突配置保持未归因。未知配置结构不作推断。缺失目录、权限不足、错误 YAML 和配置删除不阻塞用量扫描。

每个 scope/Provider 使用独立观察流，复用 passive_attribution_intervals。并发 watcher/scanner 按观察时间防止旧快照迟到后重新开启已替换路由。首次观察仅从观察时间起有效；只有同一配置再次确认才能延长，配置切换、删除及不确定状态关闭旧流，重新出现时建立新时间段。旧请求不能用今天的配置回填。即使配置重新切回原值，中间空档也不能被吞并。

## 查询与界面

新增 DeepSeek Harness 账号来源的固定 ID、过滤、排行和三语言标签。请求详情区分 `provider_reported`（会话记录的 Provider）与 `config_inferred`（配置推断）。本地扫描完成事件先补齐来源实体再刷新统计；打开来源菜单时也拉取最新实体列表；选择尚未出现在本地快照中的新来源时先补齐列表，防止筛选退化为全部。所有概览、分析、请求、会话和分享继续通过统一归因入口。账号分组不新增额度查询或代理接管能力。

## 升级与兼容

reader 指纹包含解析语义版本，防止旧版本或开发版并行写回旧指纹后跳过证据解析。SQLite v38 为旧表添加可空证据列；仅使 DeepSeek Harness 文件指纹失效，令原日志重新解析，同时清除旧物化结果。不删除请求、手动覆盖、设置或来源实体。删除 API 来源后，旧观察时间段不能继续引用已经删除的实体，历史请求回到未归因。没有 Provider 的旧格式继续未归因；已删除日志无法恢复证据。证据是本机配置归因数据，不随 WebDAV 同步传播，避免在另一台机器使用本机路由配置推断。

## 验证

覆盖 Provider 切换/缺失、压缩与 seed 兼容、账号分组、API Key 多 Provider/跨目录冲突、URL 与凭据脱敏、时间段与配置空档、手动覆盖、来源过滤、旧库迁移和历史缓存失效。运行 Rust 相关测试、npm test、npm run lint 与 git diff --check。自动化回归测试使用临时目录和数据库；原生启动检查使用正常应用数据路径。

## 依赖选择

项目此前没有 YAML 解析器；新增 [serde_yaml_ng](https://docs.rs/serde_yaml_ng/latest/serde_yaml_ng/) 解析真实 YAML，避免手写缩进、引号和重复键规则。错误只转换为固定错误码。

## 提交前审查

| 严重程度 | 位置 | 问题与修复 |
| --- | --- | --- |
| High | local_usage/database/attribution.rs · observe_deepseek_harness_passive_attribution | 完整快照保存会丢弃新来源实体；改用 update_settings_internal 原子局部注册，保留并发编辑。注册失败时关闭旧观察流。 |
| High | session/deepseek_harness_attribution.rs · compose_rows | 深度合并 config 会保留失效地址或密钥引用；按 Cordis 语义整体替换，拒绝无法确认的补丁目标和非布尔 disabled。 |
| Medium | SourceSelector.vue、monitor.ts、App.vue、DesktopApp.vue | 新来源尚未进入前端快照时，筛选可能退化为全部；菜单、选择与扫描完成事件先补齐实体列表。 |
| Medium | local_usage/database/attribution.rs · record_passive_attribution_snapshot | 并发观察可能逆序落库；拒绝迟到快照，避免重新开启旧路由或跨越配置空档。 |
| Medium | unified_usage/attribution.rs · apply_passive_attribution | 已删除的 Harness 来源可能继续保留旧 ID；缺失实体的历史请求回到未归因，并清除派生地址。 |

上述问题已修复，并补充补丁替换、禁用状态、新来源筛选、乱序观察和删除来源的回归检查。当前没有遗留阻断项。代码质量评分：8/10；不支持的外部 bundle 和动态配置保持未归因，原生窗口视觉验收受自动化工具超时限制。
