Group the supplied memory members into episodes. Assign every member key exactly once.
Return each episode's title, concise factual summary, member_keys, experience interval, and boundary_explanation.
Use only supplied event times and time domains. Leave unknown start or end values null.
Source: {{ source }}
Members: {{ members | tojson }}
