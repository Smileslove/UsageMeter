# UsageMeter 分析问题修复进度

**更新时间**: 2026-06-26  
**当前状态**: 进行中  
**处理策略**: 逐个分析 `doc/analyse/` 中的问题结论，先核实是否真实存在，再按影响面决定修复或暂缓。

---

## 已处理文件

### `06-backend-commands.md`

本轮已完成核实并落地的修复：

1. `src-tauri/src/commands/proxy.rs`
   - 已确认 `start_proxy` 在 `takeover_claude = false` 时会重复创建 `ProxyServer` 实例。
   - 已修复为单一路径启动，避免无意义实例创建和句柄覆盖。

2. `src-tauri/src/commands/updater.rs`
   - 已确认 `skip_update_version` 绕过统一设置加载/迁移/密钥持久化链路，直接改 JSON 文件。
   - 已改为复用 `load_settings` + `save_settings_internal`。
   - 对网络代理热更新失败场景做了兼容：只要设置已成功落盘，就不阻止“跳过版本”生效。

3. `src-tauri/src/commands/network_proxy.rs`
   - 已确认未知 `target` 会静默回退到 GitHub，可能让错误配置得到误导性成功结果。
   - 已改为对非法 target 返回显式错误 `ERR_INVALID_NETWORK_PROXY_TARGET`。

4. `src-tauri/src/commands/currency.rs`
   - 已确认汇率查询每次都会请求 `open.er-api.com`，没有缓存或失败回退。
   - 已新增 1 小时内存缓存。
   - 已新增失败回退逻辑：网络失败、HTTP 非成功、响应解析失败、API 非 success 时，优先返回最近一次成功缓存。

5. `src-tauri/src/commands/usage/statistics/daily_summary.rs`
   - 已确认 `success_request_count` 的成功判定与 `accumulator.rs` 不一致。
   - 原实现将 `3xx` 视为 success；现已统一为仅 `2xx` 计入 success。

6. `src-tauri/src/local_usage/database/materialized.rs`
   - 已同步修复物化日汇总与模型日汇总中的 success 判定，避免数据库快路径和实时聚合路径再次分叉。

7. `src-tauri/src/commands/usage/survival.rs`
   - 已核实 `compute_current_block` 对时间戳乱序输入缺少防御。
   - 已改为在块边界判断时拒绝“时间倒退”影响当前块起点。
   - 已同步修正块内计数逻辑，确保和新的块起点索引一致。

---

## 验证过程中新增发现并已修复

### `LocalUsageDatabase` 只读连接错库问题

文件：
- `src-tauri/src/local_usage/database/mod.rs`

结论：
- `open_readonly_connection()` 原先始终通过 `Self::db_path()` 打开全局默认数据库路径。
- 当 `LocalUsageDatabase::new_with_path(...)` 用于测试或未来自定义路径实例时，写连接和只读连接会落在不同数据库文件。
- 这会直接影响测试可信度，也会让“非默认路径实例”的读取行为错误。

修复：
- 为 `LocalUsageDatabase` 持久保存实例自己的 `db_path`。
- `open_readonly_connection()` 改为始终读取当前实例路径。

---

## 已补充的验证

已通过的定向测试：

- `cargo test --manifest-path src-tauri/Cargo.toml commands::currency`
- `cargo test --manifest-path src-tauri/Cargo.toml commands::usage::statistics::daily_summary`
- `cargo test --manifest-path src-tauri/Cargo.toml commands::usage::survival`
- `cargo test --manifest-path src-tauri/Cargo.toml unified_visible_counts_exclude_3xx_statuses`

本轮新增/加强的测试覆盖：

- `build_daily_summary_from_facts_excludes_redirects_from_success`
- `unified_visible_counts_exclude_3xx_statuses` 增补 `success_request_count` 断言
- `currency.rs` 缓存与回退逻辑单测 3 个
- `out_of_order_timestamps_do_not_move_block_backwards`

---

### `02-frontend-stores-i18n-utils.md`

本轮已完成核实并落地的修复：

1. `src/stores/updater.ts`
   - 已确认 `skipVersion()` 失败时错误码误用了 `checkFailed`。
   - 已修复为独立错误码 `skipFailed`，并把状态显式设为 `error`。

2. `src/i18n/index.ts`
   - 已新增 `settings.update.skipFailed` 的中英繁三语文案。

3. `src/components/UpdateDialog.vue`
   - 已按新的错误码区分显示 `downloadFailed` / `skipFailed` / `checkFailed`。

4. `src/components/settings/GeneralSettingsPanel.vue`
   - 已同步修正更新状态按钮中的错误文案映射，避免跳过版本失败时显示“检查失败”。

5. `src/stores/monitor.ts`
   - 已确认 `startProxy()` 在代理已成功启动但 `saveSettings()` 失败时，会留下未经校正的本地设置状态。
   - 已改为在失败后重新加载磁盘设置并刷新代理状态，而不是盲目保留或盲目回滚内存状态。

6. `src/stores/monitor.ts`
   - 已将 `invokeWithTimeout()` 的超时拒绝值从字符串改为 `Error('ERR_STATISTICS_TIMEOUT')`。
   - 已新增统一 `errorMessage()` 提取函数，减少前端直接 `String(e)` 带来的不稳定表现。

7. `src/stores/monitor.ts`
   - 已确认自动刷新仍使用固定 `setInterval`，存在请求重叠风险。
   - 已改为串行 `setTimeout` 调度，确保一轮刷新完成后才安排下一轮。

已通过的定向验证：

- `npm run build`

已核实但暂未修复：

1. `configuredSourceQuota` 请求序号逻辑冗余
   - `wantedSeq` 未实际参与后续控制流。
   - 问题存在，但需要连带复审整段队列化刷新逻辑，避免局部删变量掩盖真实并发意图。

2. `monitor.ts` 过大、订阅配额获取样板代码重复
   - 结论成立。
   - 这是结构性重构问题，不适合在当前“逐文件排雷 + 快速验证”阶段直接大拆。

3. 自动刷新缺少 `visibilitychange` 感知
   - 结论成立。
   - 需要结合主窗口/分享窗口生命周期一起设计，避免把后台暂停策略做成新的状态源。

---

### `05-backend-models-entry.md`

本轮已完成核实并落地的修复：

1. `src-tauri/src/models/settings.rs`
   - 已确认 `normalize_sync_device_id()` 中的 `Option` 包装完全冗余。
   - 已改为直接使用字符归一化值，去掉无意义的 `if let Some(...)`。

2. `src-tauri/src/models/subscription.rs`
   - 已确认 `SubscriptionQueryResult::no_credentials()` / `error()` 通过 `#[allow(unused_variables)]` 压制未使用参数告警。
   - 已改为保留 `provider` 参数并显式记录 `TODO(provider)`，同时用 `let _ = provider;` 表达当前保留意图。

3. `src-tauri/Cargo.toml`
   - 已确认 `tempfile` 仅用于测试代码，却仍位于生产依赖。
   - 已迁移到 `[dev-dependencies]`。

已通过的定向验证：

- `cargo fmt --manifest-path src-tauri/Cargo.toml`
- `cargo check --manifest-path src-tauri/Cargo.toml`

已核实但暂未修复：

1. `HttpClientFactory::init()` fallback 中的 `expect("ERR_HTTP_CLIENT_FALLBACK")`
   - 问题成立，但这是“初始化彻底失败时是否允许 panic”的启动策略问题。
   - 需要先明确全局 HTTP 客户端在极端构建失败场景下的降级目标，再决定是继续 panic 还是返回软失败路径。

2. `run()` / `monitor.ts` 等超大文件的结构性拆分
   - 结论成立。
   - 当前阶段仍以“逐条验证并修真实缺陷”为主，尚未进入大规模重构窗口。

---

### `11-build-config-cicd.md`

本轮已完成核实并落地的修复：

1. `.github/workflows/ci.yml`
   - 已确认 CI 中 Rust 工具链使用 `dtolnay/rust-toolchain@stable`，与仓库锁定的 `rust-toolchain.toml = 1.94` 不一致。
   - 已统一到 `dtolnay/rust-toolchain@1.94.0`。

2. `.github/workflows/release.yml`
   - 已同步把 Release workflow 的 Rust 工具链固定到 `1.94.0`，避免本地/CI/Release 三方版本漂移。

3. `.github/workflows/release.yml`
   - 已确认发布流程仍使用老的 `tauri-apps/tauri-action@v0`。
   - 已升级到 `tauri-apps/tauri-action@v1`，减少继续依赖旧 major 的风险。

4. `package.json`
   - 已确认项目根配置缺少 `engines` 字段。
   - 已新增 `node >= 24`、`npm >= 10` 约束，与当前 CI 环境保持一致。

5. `package.json`
   - 已新增 `npm run audit` 脚本。

6. `.github/workflows/ci.yml`
   - 已在前端 job 中加入 `npm run audit`，补上基础依赖安全扫描。

已通过的定向验证：

- `npm run build`

已核实但暂未修复：

1. `vite.config.ts` 缺少 chunk 拆分、路径别名与更细构建优化
   - 结论基本成立。
   - 但这里已经牵涉前端导入路径体系和产物体积分布，不能在未先做一轮 bundle 分析的情况下直接硬加配置。

2. `server.host` 未显式限制到 `127.0.0.1`
   - 需要先确认当前 Vite 默认行为及 Tauri dev 场景是否依赖外部访问，不宜仅凭文档结论直接改。

3. `npm audit` / `cargo audit` 的扫描阈值与流水线失败策略
   - 前端 audit 已补。
   - `cargo audit` 仍需评估是否引入额外安装依赖与误报处理策略后再加到 CI。

---

### `01-frontend-entry-architecture.md`

本轮已完成核实并落地的修复：

1. `src/main.ts`
   - 已确认前端入口没有全局 Vue 错误处理器。
   - 已补充 `app.config.errorHandler` 与 `app.config.warnHandler`，先做控制台级兜底，避免静默失败。

2. `src/theme.ts`
   - 已确认非法 `lightPalette` / `darkPalette` 值不会被校验，可能导致根节点挂载无效 palette。
   - 已新增 `sanitizeTheme()`，对浅色/深色 palette 做白名单回退。

3. `src/i18n/index.ts`
   - 已修复 `zh-TW` 翻译中的损坏 Unicode 字符：`mergeConfirm` 文案恢复正常。

已通过的定向验证：

- `npm run build`

已核实但暂未修复：

1. `App.vue` 事件监听器集中管理
   - 结论成立。
   - 但这属于可维护性重构，不是当前优先级最高的行为缺陷。

2. `types.ts` / `i18n/index.ts` 超大文件拆分
   - 结论成立。
   - 当前仍以逐条修复真实问题为主，尚未进入结构性拆分阶段。

3. `ShareWindow` 预览比例硬编码
   - 结论成立。
   - 需要结合实际视觉约束和分享窗口布局一起改，不适合在未补 UI 验证前直接调整。

---

### `07-session-readers.md`

本轮已完成核实并落地的修复：

1. `src-tauri/src/session/scanner.rs`
   - 已确认多个 `cache.lock().unwrap()` 在 mutex poisoned 时会级联 panic。
   - 已统一改为 `unwrap_or_else(|err| err.into_inner())`，避免一次异常把后续扫描链路全部拖垮。

2. `src-tauri/src/session/scanner.rs`
   - 已确认 `parse_session_file()` 会把单个会话解析失败升级为 `panic!`。
   - 已改为返回 `Result`，并在扫描聚合阶段记录错误后跳过损坏会话，不再因单文件异常导致整体崩溃。

3. `src-tauri/src/session/registry.rs`
   - 已确认 `parse_session_file_for_storage()` 同样会因解析失败直接 `panic!`。
   - 已改为返回 `Result`，由调用方决定错误处理策略。

4. `src-tauri/src/local_usage/database/scanner_sync.rs`
   - 已同步修正本地数据库扫描同步链路，对单个失败会话记录错误并跳过，而不是中断整批 dirty session 处理。

5. `src-tauri/src/session/registry.rs`
   - 已确认 `Qoder Work` / `Qoder Work CN` 虽有 reader 实现与数据库同步路径，但未注册到统一 `SessionSource` 列表，导致前端会话扫描视图不可见。
   - 已新增 `QoderWorkSource` 并接入 `all_sources()`，使其参与统一会话扫描与详情解析。

6. `src-tauri/src/session/qoder_work_reader.rs`
   - 已为 `Qoder Work` 建立与 `Hermes/Qoder IDE` 对齐的 `scan() -> cache -> parse()` 路径，避免统一扫描入口查找不到对应会话数据。

7. `src-tauri/src/session/codex_reader.rs`
   - 已确认 `normalize_model_name()` 对 UTF-8 字符串使用字节偏移切片，存在特定模型名下的 panic 风险。
   - 已改为基于 `char_indices()` 安全截取日期后缀，避免 Unicode 模型名触发崩溃。

8. `src-tauri/src/session/opencode/schema.rs`
   - 已确认 `verify_json_structure()` 仅检查 SQL 是否执行成功，没有验证计数是否大于 0。
   - 已修复为只有存在匹配的 assistant token 结构时才返回 `true`。

已通过的定向验证：

- `cargo fmt --manifest-path src-tauri/Cargo.toml`
- `cargo test --manifest-path src-tauri/Cargo.toml session::scanner -- --nocapture`
- `cargo test --manifest-path src-tauri/Cargo.toml session::codex_reader -- --nocapture`
- `cargo test --manifest-path src-tauri/Cargo.toml verify_json_structure -- --nocapture`

本轮新增/加强的测试覆盖：

- `test_parse_session_file_returns_error_for_unsupported_tool`
- `test_all_sources_registers_qoder_work_variants`
- `test_normalize_model_name_handles_unicode_without_panicking`
- `verify_json_structure_requires_matching_assistant_tokens`
- `verify_json_structure_accepts_matching_assistant_tokens`

已核实但暂未修复：

1. `scanner.rs` 增量更新在持锁状态下执行解析 I/O
   - 结论成立。
   - 需要把“解析结果构建”和“共享缓存落盘”拆成锁外准备、锁内提交两阶段，影响 `message_to_session` 去重顺序与增量替换语义，超出当前单文件缺陷修复窗口。

2. `opencode_reader.rs` 的 `parse()` 仍然通过 `scan_opencode_sessions()` 间接复用全局缓存
   - 问题成立。
   - 但 OpenCode 当前缓存状态同时承载 schema 检测、DB/file 双来源合并与 persisted checkpoint，同步改成 source 内部二级缓存需要连带审查整个状态机，不适合在本轮局部补丁中直接重构。

3. `copilot_cli_reader.rs` token 分配策略复杂且可能在 shutdown/request_count 不一致时失真
   - 结论成立。
   - 需要基于真实 Copilot CLI 样本设计更稳妥的分配规则，当前缺少足够回归样本，先保留现状。

4. `hermes_reader.rs` 时间戳字段按 `f64` 读取
   - 风险存在。
   - 但当前代码后续会统一归一化时间戳，且未见现网 schema 漂移证据；若直接改为兼容多 SQLite 类型，需要补更完整的 schema 兼容测试。

已核实后不按文档建议修复：

1. `codex_reader.rs` fork replay 判定中的 `ts <= fork_start_ts`
   - 文档建议改成 `<`，但结合当前 Codex fork 语义与现有回归测试，这会把 fork 创建时刻批量回放的历史事件重新计入统计，造成重复计算。
   - 当前实现应保留 `<=`，否则会回退已修好的 fork 去重行为。

---

### `03-frontend-views.md`

本轮已完成核实并落地的修复：

1. `src/views/Sessions.vue`
   - 已确认最近会话卡片里仍有硬编码 `'Unknown'`。
   - 已改为统一使用 `t(store.settings.locale, 'common.unknown')`。

2. `src/views/Statistics.vue`
   - 已确认自定义范围防抖定时器 `customRangeTimer` 在组件卸载时未清理。
   - 已补充 `onUnmounted` 清理逻辑。

3. `src/views/Statistics.vue`
   - 已确认初始化阶段会在异步数据请求完成前把 `initialized` 置为 `true`。
   - 已改为在 `Promise.all([fetchSummary(), fetchMonth()])` 完成后再置位。

已通过的定向验证：

- `npm run build`

已核实但暂未修复：

1. `Statistics.vue` 的初始化时序和自定义范围定时器清理
   - 问题结论需要结合实际交互与组件卸载路径一起验证。
   - 当前还未发现明确的运行时错误证据，先不做“为修而修”的异步改写。

2. `Sessions.vue` 的超大模板/脚本拆分
   - 结论成立。
   - 这是结构性重构问题，不适合在当前逐条缺陷修复阶段直接展开。

---

### `04-frontend-components.md`

本轮已完成核实并落地的修复：

1. `src/components/ModelDistribution.vue`
   - 已确认中心文案 `Models` 为硬编码。
   - 已改为使用 `metrics.modelDistribution` 的 i18n 文案。

2. `src/components/SessionDetailModal.vue`
   - 已确认多个字段标签仍保留中文 fallback（`工作目录`、`最后提示`、`错误`、`开始时间`、`结束时间`）。
   - 已全部改为直接依赖现有 i18n key，不再保留自然语言兜底。

3. `src/components/ModelPricingSettings.vue`
   - 已确认价格列表中 `输入:` / `输出:` / `缓存读:` / `缓存写:` 为硬编码。
   - 已统一改为 `settings.modelPricingInput` / `Output` / `CacheRead` / `CacheWrite`。

4. `src/components/ModelPricingEditModal.vue`
   - 已确认 `common.cancel` 仍带 `'取消'` fallback。
   - 已移除 fallback，直接走 i18n。

5. `src/components/ModelPricingEditModal.vue`
   - 已确认汇率读取仍使用 `|| 1.0`。
   - 已统一改为 `?? 1.0`，避免把显式 `0` 与“未配置”混为一谈。

6. `src/components/DynamicIcon.vue`
   - 已确认未知图标名会静默 fallback 到 `Globe`，缺少诊断信息。
   - 已在开发环境增加 `console.warn` 提示。

7. `src/components/SessionDetailModal.vue`
   - 已补充 `Escape` 键关闭支持。

已通过的定向验证：

- `npm run build`

已核实但暂未修复：

1. `DynamicIcon.vue` 的类型安全与 fallback 诊断
   - 问题成立。
   - 需要先确认项目是否接受在运行期输出 icon fallback 警告，避免污染正常日志。

2. `UpdateDialog.vue` 的 markdown / `v-html` 安全策略
   - 当前实现存在人工转义，是否升级到 markdown 库 + sanitizer 需要单独评估依赖与兼容性。

3. 多个超大组件与重复 CSS 抽取
   - 结论成立。
   - 属于后续可维护性重构，不是本轮优先处理的确定性行为缺陷。

---

### `08-proxy-system.md`

本轮已完成核实并落地的修复：

1. `src-tauri/src/proxy/source_detector.rs`
   - 已确认 `normalize_base_url()` 通过 `contains("api.anthropic.com")` 判定官方地址，匹配范围过宽。
   - 已改为按 host 精确识别 `api.anthropic.com`，避免把 `api.anthropic.com.evil.example` 或路径中包含该字符串的第三方地址误归并为官方源。

2. `src-tauri/src/proxy/collector.rs`
   - 已确认 `UsageCollector::record()` 中 `is_duplicate` 分支在数据库写入阶段执行完全相同逻辑，判断结果没有实际意义。
   - 已移除无效分支，保留“内存去重/替换 + 数据库存储”语义不变，减少误导性控制流。

3. `src-tauri/src/proxy/openai_forwarder.rs`
   - 已确认 OpenAI/Codex usage 缺少上游 message id 时，fallback id 通过 `Instant::now().elapsed().as_nanos()` 生成，熵极低且语义错误。
   - 已改为使用进程内原子递增计数器生成稳定唯一的 fallback message id，避免近零值导致的潜在去重冲突。

已通过的定向验证：

- `cargo fmt --manifest-path src-tauri/Cargo.toml`
- `cargo test --manifest-path src-tauri/Cargo.toml proxy::source_detector -- --nocapture`
- `cargo test --manifest-path src-tauri/Cargo.toml proxy::collector -- --nocapture`
- `cargo test --manifest-path src-tauri/Cargo.toml fallback_message_ids_are_unique_without_usage_ids -- --nocapture`

本轮新增/加强的测试覆盖：

- `test_normalize_base_url` 增补端口、大小写与恶意相似域名场景
- `recent_duplicate_replaces_cached_snapshot`
- `fallback_message_ids_are_unique_without_usage_ids`

已核实但暂未修复：

1. `openai_forwarder.rs` / `gemini_forwarder.rs` 显式 `pool_max_idle_per_host(0)`
   - 问题成立，会关闭 keep-alive 连接复用。
   - 但这里与当前 `http1_only`、流式连接隔离和部分上游兼容性绑定在一起，直接放开连接池需要补更完整的代理实测与回归，不适合在本轮局部补丁中贸然修改。

2. 多个 ConfigManager / Registry 直接 `fs::write(...)`
   - 非原子写入风险成立。
   - 但这是跨 Claude/Codex/OpenCode/Reasonix/Gemini 的一致性修复，需统一设计 write-then-rename 策略和 Windows/macOS 差异处理，本轮先不拆散做半套修补。

3. `ProxySourceRegistry` 的 Claude handle id 仍基于完整 settings snapshot
   - 结论成立。
   - 但这会影响既有 handle 稳定性与恢复路径，需要连带评估迁移兼容和旧 registry 数据归并，当前先保留现状，后续单独处理。

4. OpenCode `Unknown` 协议默认走 OpenAI passthrough
   - 结论成立。
   - 需要基于真实未知 provider 样本定义协议推断策略，否则容易引入新的误判路径，本轮不做推测式修复。

---

### `09-database-unified-usage.md`

本轮已完成核实并落地的修复：

1. `src-tauri/src/local_usage/database/mod.rs`
   - 已确认 `ensure_synced_throttled()` 在 `Mutex` 或 `Condvar::wait()` 遇到 poisoned 状态时会直接 `unwrap()` panic。
   - 已统一改为 `unwrap_or_else(|err| err.into_inner())`，避免同步门控在前序异常后把后续数据库同步链路全部拖垮。

2. `src-tauri/src/local_usage/database/mod.rs`
   - 已新增统一的 `saturating_i64_to_u64()` 转换辅助函数。
   - 用于把数据库层读取到的异常负值安全钳制到 `0`，避免裸 `as u64` 产生巨大的 wraparound 数值。

3. `src-tauri/src/local_usage/database/queries.rs`
   - 已确认本地请求/会话查询中多处直接使用 `row.get::<_, i64>(...) as u64`。
   - 已改为统一走饱和转换，覆盖 `input/output/cache/total/reasoning/message_count/file_size` 等字段。

4. `src-tauri/src/local_usage/database/remote_sync.rs`
   - 已同步修复远程同步导出、远程请求查询和远程会话查询中的同类 `i64 -> u64` 不安全转换。
   - 现在远程数据即使遇到脏值，也会被安全归零而不是变成超大正数。

5. `src-tauri/src/unified_usage/service.rs`
   - 已确认 `request_key_for_fact()` 存在一段没有实际效果的重复分支，前后返回格式完全相同。
   - 已删除冗余条件，保留 canonical key 优先和默认格式化逻辑不变，减少误导性控制流。

已通过的定向验证：

- `cargo fmt --manifest-path src-tauri/Cargo.toml`
- `cargo test --manifest-path src-tauri/Cargo.toml local_request_query_saturates_negative_token_values -- --nocapture`
- `cargo test --manifest-path src-tauri/Cargo.toml remote_request_query_saturates_negative_token_values -- --nocapture`

本轮新增/加强的测试覆盖：

- `local_request_query_saturates_negative_token_values`
- `remote_request_query_saturates_negative_token_values`

验证中发现但未在本轮处理的既有问题：

1. `local_usage::database::tests` 中多条 `opencode_*` 用例当前失败
   - 复现命令：
     `cargo test --manifest-path src-tauri/Cargo.toml opencode_db_checkpoint_persists_across_reopen -- --nocapture --test-threads=1`
   - 失败表现是 `get_request_records_in_range(...)` 返回 0 条，而测试预期 2 条，并进一步触发测试互斥锁 poisoned。
   - 这组失败发生在本轮新增定向测试之外，且与本轮修改的负值饱和转换无直接因果证据；需在后续专门排查 Opencode 同步/状态恢复链路时单独处理。

已核实但暂未修复：

1. `unified_usage/service.rs` 的 `acquire_inflight_key()` 轮询等待没有超时
   - 风险描述成立。
   - 但当前 guard 在正常 drop 路径会释放 key，是否引入超时需要先明确调用方对“放弃合并任务”的容忍语义，否则容易把一次慢任务误判成失败。

2. 数据库完整性检查与迁移前备份机制
   - 问题成立。
   - 但这属于数据库生命周期策略增强，不适合在当前逐文件缺陷修复阶段直接插入新的启动/迁移行为。

---

### `10-subscription-system.md`

本轮已完成核实并落地的修复：

1. `src-tauri/src/subscription/mod.rs`
   - 已确认订阅缓存此前仅使用 `provider` 作为 key。
   - 对 `relay` 与 `source-config` 这类多实例结果，会出现后一次查询覆盖前一次查询的问题。
   - 已改为按 `provider + tool + source_tool` 生成稳定缓存 key，并在按 provider 读取缓存时返回该 provider 下最新的一条有效结果，避免不同来源额度互相污染。

2. `src-tauri/src/subscription/mod.rs`
   - 已同步修正 `clear_cache(provider)` 的清理逻辑，使其按 provider 前缀清除整组缓存，而不是只删除旧的单键形式。

3. `src-tauri/src/copilot/auth.rs`
   - 已确认 `CopilotAuthManager::new(data_dir)` 完全忽略传入参数，始终硬编码到 `~/.config/github-copilot`。
   - 已改为优先使用传入的 `data_dir/github-copilot` 作为凭据目录，保留空路径时回退到原默认目录的兼容行为。

4. `src-tauri/src/subscription/gpt.rs`
   - 已确认 GPT 额度查询在错误分支会把原始响应体直接打印到 stderr。
   - 已移除调试输出，避免把上游错误响应中的敏感上下文泄漏到日志。

已通过的定向验证：

- `cargo fmt --manifest-path src-tauri/Cargo.toml`
- `cargo test --manifest-path src-tauri/Cargo.toml cache_separates_relay_and_source_tool_variants -- --nocapture`
- `cargo test --manifest-path src-tauri/Cargo.toml new_uses_passed_data_dir -- --nocapture`
- `cargo test --manifest-path src-tauri/Cargo.toml subscription:: -- --nocapture`

本轮新增/加强的测试覆盖：

- `subscription::tests::cache_separates_relay_and_source_tool_variants`
- `copilot::auth::tests::new_uses_passed_data_dir`

---

### `12-overall-summary.md`

本轮已完成核实并落地的修复：

1. `src-tauri/tauri.conf.json`
   - 已确认生产配置中 `app.security.csp` 仍为 `null`，会完全关闭 WebView 的内容安全策略。
   - 已按当前资源使用面补上最小可用 CSP：限制脚本只能来自应用自身，限制连接仅允许 Tauri 本地 IPC/asset 通道，并仅对白名单图片来源开放 `data:` / `blob:` / `https://unpkg.com`。
   - 开发配置 `src-tauri/tauri.conf.dev.json` 维持现状，避免直接影响 `tauri dev` 的本地调试体验。

已通过的定向验证：

- `npm run build`

已核实但暂未修复：

1. `src/App.vue` 仍使用 `v-if` / `v-else-if` 切换顶级视图
   - 问题成立，当前切换视图会销毁并重建组件实例。
   - 但这会直接改变 Overview / Statistics / Sessions / Settings 的生命周期与刷新时机，需要补一轮视图级行为回归后再决定是否改成 `<KeepAlive>`。

2. `src-tauri/src/local_usage/database/migrations.rs` 在多个历史迁移里重复调用 `create_unified_materialized_tables()`
   - 问题成立。
   - 不过这些调用位于既有 schema 升级路径中，当前实现虽然冗余，但依赖 `IF NOT EXISTS` 保持幂等；若现在折叠迁移步骤，需要额外验证旧版本数据库跨多个版本升级的完整路径，当前先保留。

3. 前端 / 代理 / 测试体系相关的大规模结构性建议
   - `monitor.ts`、`Sessions.vue`、代理子系统体量过大以及测试覆盖偏弱等结论仍然成立。
   - 这些属于专项重构或测试建设议题，不适合作为本轮 `12-overall-summary.md` 的单独补丁处理。

---

### `A-backend-commands-deep.md`

本轮已完成核实并落地的修复：

1. `src-tauri/src/lib.rs`
   - 已确认 macOS `make_window_rounded()` 中对 `window.ns_window()` 的直接取值存在崩溃风险。
   - 已改为在原生窗口句柄获取失败时记录错误并提前返回，不再因圆角窗口增强失败导致应用 panic。

2. `src-tauri/src/lib.rs`
   - 已确认后台更新检查把 `Update` 写入 `pending_update` 时仍直接 `lock().unwrap()`。
   - 已改为在 mutex poisoned 时恢复内部值继续执行，避免前序异常把后续更新检查链路再次拖崩。

3. `src-tauri/src/commands/updater.rs`
   - 已确认 `check_for_update()`、`download_and_install_update()`、`skip_update_version()` 对 `pending_update` 均仍使用 `lock().unwrap()`。
   - 已统一改为 `unwrap_or_else(|err| err.into_inner())`，保证 poisoned mutex 只降级为状态恢复，不再演变为新的 panic。

4. `src-tauri/src/commands/settings.rs`
   - 已确认设置保存仍直接 `fs::write(settings.json)`，写盘中断时可能留下半写入文件。
   - 已改为先写入同目录临时文件再 `rename` 覆盖目标文件，补上基础原子写盘保障。

已通过的定向验证：

- `cargo fmt --manifest-path src-tauri/Cargo.toml`
- `cargo test --manifest-path src-tauri/Cargo.toml commands::updater::tests -- --nocapture`
- `cargo test --manifest-path src-tauri/Cargo.toml commands::settings::tests -- --nocapture`

本轮新增/加强的测试覆盖：

- `commands::settings::tests::atomic_write_replaces_target_file_contents`

已核实但不按文档结论修复：

1. `src-tauri/src/commands/window.rs` 重复打开分享窗口
   - 当前实现已先通过 `get_webview_window("share")` 复用现有窗口，不会重复创建。
   - 文档结论与现代码不符，不做修改。

2. `src-tauri/src/proxy/database/pricing.rs` 批量定价“无事务”
   - 当前实现已在 `apply_pricing_to_records()` 中显式开启 `transaction()` 并在同一事务内批量更新与刷新日汇总。
   - 文档结论与现代码不符，不做修改。

3. `src-tauri/src/commands/model_pricing.rs` 搜索无分页
   - 当前 `search_model_pricing()` 已接收 `limit` / `offset`，默认值为 `100 / 0`。
   - 文档结论与现代码不符，不做修改。

4. `src-tauri/src/unified_usage/service.rs` 缓存无限增长
   - 当前各运行时缓存已设置固定容量常量并在存储时裁剪。
   - 文档结论与现代码不符，不做修改。

已核实但暂未修复：

1. `lib.rs` 的 `.run(...).expect(...)`
   - 问题成立，但这里位于应用主入口，改成“非 panic”后也无法在同一进程内继续运行。
   - 若要优化，只能改成更友好的启动失败上报/日志策略，不属于当前局部补丁范围。

2. `menu_labels()` 仍是本地硬编码的双语菜单标签
   - 问题成立。
   - 但托盘菜单构建发生在 Rust 侧启动流程，若要接入统一 i18n，需要先定义一套后端可消费的稳定 locale 字典或共享翻译导出机制，当前先保留。

3. `load_settings()` 在多条命令路径中重复读盘
   - 结论成立。
   - 这会牵涉设置缓存、热更新、secret hydrate/persist、一致性失效策略等整套状态模型，当前不做局部缓存补丁。

4. 全局 `eprintln!` 与统一错误类型问题
   - 结论成立。
   - 但这属于跨模块基础设施收敛，影响范围远大于单文件缺陷修复，本轮先只修确定性的 panic / 数据落盘风险。

---

### `A-cross-validation.md`

本轮已完成核实并落地的修复：

1. `src/iconConfig.ts`
   - 已确认 `SOURCE_ICON_CATEGORIES` 仍包含 8 个中文类目标签硬编码，违反 i18n 约束。
   - 已改为在配置层保存 i18n key，由组件渲染时再翻译，避免把自然语言常量继续保留在业务代码里。

2. `src/components/ApiSourceList.vue`
   - 已同步改为按 `labelKey` 渲染图标类目标题，搜索筛选时保留原有行为不变。

3. `src/i18n/index.ts`
   - 已补充图标类目标题的中英繁三语文案。

已通过的定向验证：

- `npm run build`

已核实后确认前序分析需要修正的点：

1. 后端测试覆盖评估偏低
   - 当前仓库存在大量 Rust 内联单测，`rg -n '#\\[test\\]|mod tests' src-tauri/src` 统计结果远高于“仅 1 个测试文件”的原始说法。
   - 更准确的表述应是：后端已有较多内联单测，但独立测试文件较少；前端测试仍基本缺失。

2. `src/components/UpdateDialog.vue` 的 `v-html` 风险需要降级描述
   - 当前实现不是把原始 release notes 直接塞进 DOM，而是先经过 `escapeHtml()` 与受限 markdown 渲染。
   - 这仍属于“依赖手写 sanitizer 的安全敏感点”，但不能按“现成的裸 XSS 漏洞”表述。

3. 交叉审计列出的多项严重遗漏，当前已在前几轮完成修复
   - 包括 Session panic / mutex poisoning / Qoder Work 集成、Proxy URL contains 误判、Subscription 缓存键冲突、GPT stderr 敏感日志、CSP 禁用等。
   - `A-cross-validation.md` 在当前仓库状态下更适合作为“复盘修正报告”，而不是新增缺陷清单。

已核实但暂未修复：

1. `DynamicIcon` 全量导入 `lucide-vue-next`
   - 风险成立，属于前端包体积优化议题。
   - 但是否改为显式按需映射需要先评估现有图标动态性和维护成本，本轮先不动。

2. `StatisticsModelList.vue` tooltip formatter 直接拼接 HTML 字符串
   - 当前模型名来源并未见明确的 HTML 转义保障。
   - 但这条需要结合 ECharts tooltip 渲染方式和真实模型名输入面一起验证，避免在未确认攻击面前做推断式修复。

---

### `A-frontend-components-deep.md`

本轮已完成核实并落地的修复：

1. `src/components/ModelPricingSettings.vue`
   - 已确认自定义/同步模型搜索防抖定时器在组件卸载时未清理。
   - 已补充 `onUnmounted` 清理 `customSearchTimeout` / `syncedSearchTimeout`，避免组件销毁后残留异步回调。

2. `src/components/CurrencySettings.vue`
   - 已确认汇率同步成功提示的 `setTimeout` 未跟踪和清理。
   - 已改为持有 `syncSuccessTimer` 并在卸载时清理。

3. `src/components/settings/NetworkProxyPanel.vue`
   - 已确认“已保存”闪烁提示的 `setTimeout` 未清理。
   - 已改为持有 `npSavedFlashTimer` 并在卸载时清理。

4. `src/components/settings/WslScanPanel.vue`
   - 已确认高级设置保存成功提示的 `setTimeout` 未清理。
   - 已改为持有 `wsSavedFlashTimer` 并在卸载时清理。

5. `src/components/statistics/StatisticsModelList.vue`
   - 已确认 donut tooltip 会把 `model.modelName` 直接拼接进 HTML 字符串。
   - 已新增最小 HTML 转义，避免模型名称中的特殊字符被直接解释为标签。

已通过的定向验证：

- `npm run build`

已核实后不按文档结论修复：

1. `ModelDistribution.vue` / `SessionDetailModal.vue` / `ModelPricingSettings.vue` 中多处硬编码文案
   - 这些问题已在 `04-frontend-components.md` 轮次完成修复。
   - `A-frontend-components-deep.md` 这里反映的是旧状态，不再重复修改。

2. `UpdateDialog.vue` 的 `v-html` 结论
   - 当前实现会先经过 `escapeHtml()` 与受限 markdown 渲染，不是把原始远端内容直接注入 DOM。
   - 这仍是安全敏感点，但不能按“现成裸 XSS”定性；已在 `A-cross-validation.md` 轮次中同步修正文档结论。

已核实但暂未修复：

1. `src/components/DynamicIcon.vue` 全量导入 `lucide-vue-next`
   - 风险成立。
   - 但要改成按需映射，需要先盘点调用端的动态图标集合和回退策略，当前先不做包体积方向的大改。

2. 组件层系统性 ARIA/键盘导航补齐
   - 结论成立，尤其是多个模态框和选择器缺少完整的无障碍语义。
   - 但这是跨多组件的一致性工程，当前先修确定性的生命周期缺陷和 HTML 注入点。

3. `StatisticsModelList.vue` / `StatisticsTrendChart.vue` 的深色模式颜色与 tooltip 样式硬编码
   - 风险成立。
   - 需要和现有主题变量体系一起重构，避免只改局部颜色导致风格割裂，本轮先不展开。

---

## 已核实但暂未修复

### `get_recent_request_records` 全量加载再分页

文件：
- `src-tauri/src/commands/usage/requests.rs`

结论：
- 问题真实存在，当前仍通过 `get_merged_request_facts(..., None, None, ...)` 拉取全部历史事实后排序分页。

暂缓原因：
- 若要彻底修正，需要把“最近请求”的排序/分页能力下推到统一合并层，并正确处理：
  - 本地物化历史数据
  - 当日热数据
  - 代理/本地去重合并后的统一排序
  - 现有 tool/source filter 语义
- 这不是一个适合在未补齐更大范围验证面的情况下做的局部小改。

后续建议：
- 为 unified usage 增加“按时间倒序分页读取 merged facts”的专用查询入口，再让 command 层改用该入口。

---

## 下一步

1. 继续处理 `06-backend-commands.md` 中剩余仍成立且可独立闭环的问题。
2. 进入下一个未处理分析文件，重复执行：
   - 阅读实现
   - 核实问题真伪
   - 判断是否需要修复
   - 实施并验证
3. 所有分析文件处理完成后，再统一做：
   - 总体验证
   - 最终总结文档整理
   - git 提交信息生成与提交
