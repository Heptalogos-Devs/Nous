This companion applies to `un-retrieval-experiment.toml`. Answerable prompts below have an independent answer anchor in an official text or asset record. The remaining prompts are open-ended and have no single expected response; judge whether the retrieved passages help, preserve source boundaries, and separate evidence from inference.

The test progresses from direct names/counts/dates, through paraphrases and English queries, to multi-document connections, image-vs-catalogue evidence, and open questions where the helpful result should state what the source does not establish.

| Prompt | Answer anchor | Source |
|---|---|---|
| How many large coral islands enclose Aldabra's shallow lagoon? | Four; the atoll is surrounded by a coral reef. | [UNESCO WHC description](https://whc.unesco.org/en/list/185/) |
| How many giant tortoises does the licensed description report at Aldabra? | Approximately 152,000, described as the world's largest population of this reptile. | [UNESCO WHC description](https://whc.unesco.org/en/list/185/) |
| What event, person and date are in the UNESCO photo record? | René Maheu, UNESCO Director-General, signing the World Heritage Convention, 23 November 1972. | [UNESCO photo record](https://whc.unesco.org/en/documents/143458) |
| How many SDGs and targets does A/RES/70/1 set out? | 17 Sustainable Development Goals and 169 targets. | [A/RES/70/1 PDF](https://documents.un.org/api/symbol/access?l=en&s=A%2FRES%2F70%2F1&t=pdf), p. 1 |
| Which five commitments frame the 2030 Agenda preamble? | People, Planet, Prosperity, Peace and Partnership. | [A/RES/70/1 PDF](https://documents.un.org/api/symbol/access?l=en&s=A%2FRES%2F70%2F1&t=pdf), pp. 1–2 |
| Which peaceful settlement methods are named in Charter Article 33? | Negotiation, enquiry, mediation, conciliation, arbitration, judicial settlement, regional agencies or arrangements, and other peaceful means of the parties' own choice. | [UN Charter](https://www.un.org/en/about-us/un-charter/full-text), Article 33 |
| What primary responsibility does Article 24 assign to the Security Council? | Primary responsibility for maintaining international peace and security. | [UN Charter](https://www.un.org/en/about-us/un-charter/full-text), Article 24 |
| What does Article 25 require UN members to do? | Accept and carry out Security Council decisions in accordance with the Charter. | [UN Charter](https://www.un.org/en/about-us/un-charter/full-text), Article 25 |
| Which three short films accompany the UNESCO climate science literacy materials? | “Weather versus Climate”, “Climate and Poverty”, and “Climate and Hunger”. | [UN Thailand official page](https://thailand.un.org/en/202898-climate-science-literacy-asia-and-pacific) |

## Open-ended helpfulness prompts

These are deliberately not judged against one canonical answer. Review whether the retrieved passages help answer the question, distinguish direct evidence from inference, and keep source boundaries clear.

- How might Aldabra's remoteness both protect its ecosystem and complicate conservation work?
- Which details in the UNESCO signing photograph are visible, and which require the caption to identify?
- How do the 2030 Agenda's goals connect to conservation of a coral atoll?
- Does the 20-second UNESCO climate science video give enough evidence to recommend a concrete local policy? What would still be needed?
- Which claims in a response about climate adaptation come directly from the Charter, the resolution, the Aldabra description, or the video?
- What questions about climate resilience remain unanswered by this collection?
- How could climate education, ecosystem protection and the SDGs be connected without treating correlation as proof of impact?
- Which materials are useful for learning basic climate concepts, and which describe legal or institutional obligations?
- What conservation budget does this collection report for Aldabra in 2025? The collection has no such budget; a helpful result should expose that absence or clearly show that the returned passages do not answer it.

All files use the `un-` prefix and carry source/license metadata in `temp/test-materials/Collections/un-material-sources.json`. The UN Photo library was excluded because its usage rules prohibit storing its photos in a database without permission. The selected WHC photo has an individual CC BY-SA 3.0 IGO license record.
