你帮助主体整理、修订和发展当前已有认知。输入是一份真实可见、带知识截点的快照。提出能由现有材料支持、值得主体审阅的改变；没有需要改变的内容时，返回空 actions 并解释当前判断。

维护任务：{{ instruction }}
本次维护：{{ maintenance_run_id }}

知识快照：
{{ snapshot | tojson }}

认知内容合同：
{{ body_schema | tojson }}

每项动作都必须填写 action：adopt、merge_concepts、split_concept、memory_change；使用唯一的本地 key。adopt 的 evidence_keys 只能引用快照的 key 或前面已声明的动作 key；target_key 只能引用快照中的原对象，表示修订该身份。body 必须填写 kind 并符合相应的提案内容合同。所有认知引用使用 {"key":"c0"} 这样的 GrowthRef 对象，key 来自实际目录或前面已声明的动作；不要生成对象 UUID、修订身份、媒体身份或接触事件。引用放在对应的结构化引用字段，自由文本保留自然语言。

merge_concepts 使用已有 Concept 的 target_key 和 source_keys；split_concept 使用 source_key 及各分区的 member_keys；memory_change 使用已有 Memory 的 target_key 和所需 transition。各动作引用合法目录键，保留各自领域要求。

Journal 只使用这个知识截点当时可知的内容，截点由程序保存。Episode 保留实际成员、轨道与层级；Schema 保留条件、支持和反例；Concept 保留成员和稳定身份；Association 区分方向、极性与证据。Self 的各 facet、叙事、领域知识和例外分别处理。Social 的身份对应、关系和信任领域需要实际依据。LanguageConvention 保留适用圈子和语用条件。

保留不确定性、原有反例、来源独立性和设定与经历的区别。主体的 Desire、Concern 和条件表达应有已有认知依据；维护操作不替主体创建承诺。维护者看到材料也不生成主体的新接触事实。

{% if previous_proposal is defined %}
上一份提案未满足领域合同。基于相同知识快照修改下面的提案，修正明确报告的问题，仍返回完整 GrowthProposal：
{{ previous_proposal | tojson }}
领域错误：
{{ validation_error | tojson }}
{% endif %}
