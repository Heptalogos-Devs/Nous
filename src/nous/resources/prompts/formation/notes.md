从给定片段或附带的媒体中提炼以后有用的认识。没有值得保留的认识时返回空 notes。片段带有 s 开头的短键；source_keys 只填写直接支持该条认识的片段短键。背景用于理解，不要把它当作新的事实来源。保留作者、说话者、条件、否定和不确定性，转述不要写成亲历。区分直接可见或可听到的内容与字幕或叙述中的主张。

旧认识带有 m 开头的短键。先判断新的陈述与旧认识是否描述同一事项、同一认识归属和适用范围，再选择关系：new 是新事项；same 是同一认识的重复支持；supplement 是补充；changed 是原事项的后继状态；conflict 是不同主张或条件下相互冲突。new 的 previous_key 为 null，其余填写对应的 m 短键。不要仅因时间较新或名字相同就认定 changed；不同人的说法可以同时保留。过去某时有人说过的话仍是历史事实，不因后来改变计划而消失。每条 text 应能独立理解，topics 是少量简短主题词，time_expression 只写来源明示的时间。不要输出程序身份或操作。

例：旧认识 [m1]「子代理使用 Terra xhigh」。新片段 [s1]「将子代理改为 Terra max」。输出 {"notes":[{"text":"子代理使用 Terra max。","topics":["子代理","模型"],"source_keys":["s1"],"time_expression":null,"previous_key":"m1","relation":"changed"}]}。
例：旧认识 [m1]「林计划春天搬家」。新片段 [s1]「林说还没有确定是否搬家」。输出 {"notes":[{"text":"林说还没有确定是否搬家。","topics":["林","搬家"],"source_keys":["s1"],"time_expression":null,"previous_key":"m1","relation":"changed"}]}。
例：新片段 [s1]「林说她没有搬到大阪，如果调动通过，明年春天可能搬去」。没有旧候选时输出两条，各自使用 source_keys ["s1"]、previous_key null、relation "new"，保留否定、条件和可能性。

<!-- input -->
本次来源：
{{ source_text }}
{% if background %}
必要背景：
{{ background }}
{% endif %}
{% if previous_memories %}
已有认识：
{{ previous_memories }}
{% endif %}
