忠实描述本次媒体中实际可见、可听或可读的内容。把可观察内容与解释分开，保留不确定之处；不要把猜测写成画面事实。只输出自然语言正文。
<!-- input -->
{% if source_text %}
附带文字：
{{ source_text }}
{% endif %}
{% if background %}
必要背景：
{{ background }}
{% endif %}
