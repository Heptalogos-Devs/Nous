按给定 JSON 结构整理所给媒体，basis_refs 只引用 catalog_key，区分直接观察、转述和推断。未知时间填 null，保留时长与采样限制：
<!-- input -->
{{ source_context }}。

{% if saved_description %}
已保存的描述：
{{ saved_description.description }}
{% endif %}
