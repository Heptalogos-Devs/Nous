从 source_text 提炼 1 到 3 条可独立理解的记忆，保留人物、时间、归属、条件和不确定性，不添加来源外事实。source 标明实际输入原件、区域或派生表示；source_event_kind 区分实际接触与导入。描述、解释和转述保持其归属，不写成主体直接观察所得。
按给出的 JSON 结构输出 title、content、epistemic_status。转述资料用 reported，解释推断用 inferred，尚未确定用 uncertain。使用材料的语言。
