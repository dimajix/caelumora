
gemma4:26b: texts = answers with text; wrong = mentions an agent whose answer is not wanted

| variant | social: texts with a mention | social: mentions per text | social: said nothing | work: wrong mention | handover: mentions whom it hands to | handover: wrong mention |
|---|---:|---:|---:|---:|---:|---:|
| current | 23/24 | 2.8 | 0/24 | 1/2 | 3/4 | 3/4 |
| strong | 14/23 | 2.0 | 1/24 | 0/2 | 4/4 | 2/4 |
| turn | 9/23 | 0.8 | 1/24 | 0/2 | 4/4 | 2/4 |
| strong_turn | 7/24 | 0.5 | 0/24 | 0/2 | 4/4 | 0/4 |
| names_strong_turn | 6/24 | 0.8 | 0/24 | 1/2 | 4/4 | 1/4 |
| clean | 19/23 | 1.6 | 1/24 | 0/2 | 4/4 | 3/4 |
| clean_strong_turn | 1/23 | 0.0 | 1/24 | 0/2 | 4/4 | 0/4 |
| rules_names | 16/23 | 1.9 | 0/24 | 0/2 | 4/4 | 4/4 |
| rules_names_turn | 8/24 | 0.7 | 0/24 | 0/2 | 4/4 | 1/4 |
| cond_names | 23/24 | 2.4 | 0/24 | 0/2 | 4/4 | 3/4 |
| test_names | 16/23 | 2.3 | 1/24 | 0/2 | 4/4 | 4/4 |
| test_names_turn | 3/24 | 0.2 | 0/24 | 0/2 | 3/4 | 0/4 |

Did not end before the limit, or failed: 1 - Coder:13 names_strong_turn (runaway)

Per situation (agents mentioned per answer; - none, q silence, w tool only):

| situation | kind | current | strong | turn | strong_turn | names_strong_turn | clean | clean_strong_turn | rules_names | rules_names_turn | cond_names | test_names | test_names_turn |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| Coder:6 | social | jonathan,sassy / jonathan,planner,reviewer,sassy | jonathan,sassy / - | - / - | - / - | - / - | - / jonathan,sassy | - / - | - / jonathan,sassy | - / - | jonathan,sassy / jonathan,planner,reviewer,sassy | jonathan,planner,reviewer,sassy / - | - / - |
| Planner:focus | handover | coder,reviewer,sassy / - | coder,reviewer / coder | coder / coder,reviewer | coder / coder | coder / coder,reviewer | coder,reviewer,sassy / coder | coder / coder | coder,sassy / coder,jonathan,reviewer,sassy | coder / coder,reviewer | coder,jonathan,sassy / coder,reviewer,sassy | coder,reviewer / coder,reviewer | coder / coder |
| Coder:0 | social | planner / planner | - / - | - / planner | planner / - | - / - | planner / planner | - / - | planner / - | - / - | planner / planner | planner / - | - / - |
| Reviewer:0 | social | planner / planner | - / - | - / - | - / - | - / - | coder,planner / planner | - / - | - / - | - / - | planner / - | - / - | - / - |
| Planner:1 | social | reviewer / reviewer | - / - | - / - | - / - | - / - | reviewer / reviewer | - / - | - / w | - / - | reviewer / reviewer | - / - | - / - |
| Reviewer:3 | social | coder,planner / coder,planner | coder,planner / coder,planner | planner / - | - / - | - / - | coder,planner / coder,planner | - / - | coder,planner / coder,planner | - / - | coder,planner / coder,planner | - / coder,planner | - / - |
| Coder:8 | social | jonathan,planner,reviewer,sassy / jonathan,sassy | jonathan,sassy / jonathan,planner,reviewer,sassy | jonathan,sassy / jonathan,planner,reviewer,sassy | reviewer,sassy / jonathan,sassy | planner,reviewer,sassy / - | planner / jonathan,planner,reviewer,sassy | - / - | jonathan,reviewer,sassy / jonathan,planner,reviewer,sassy | - / planner | planner,sassy / jonathan,planner,reviewer,sassy | jonathan,sassy / jonathan,planner,reviewer,sassy | - / - |
| Jonathan:2 | social | coder,reviewer,sassy / coder,sassy | - / coder,reviewer | - / - | - / - | - / - | - / - | - / - | coder,reviewer / coder,reviewer | - / - | coder,reviewer / coder,sassy | - / coder,reviewer | coder / - |
| Planner:12 | social | - / all,coder,jonathan,reviewer,sassy | - / coder,jonathan,reviewer,sassy | - / - | - / sassy | - / coder,jonathan,reviewer,sassy | - / jonathan | - / - | jonathan,sassy / - | - / coder,jonathan,reviewer,sassy | coder,jonathan,reviewer / coder,jonathan,reviewer,sassy | coder,jonathan,reviewer,sassy / coder,jonathan,sassy | - / - |
| Jonathan:5 | social | coder,planner / coder,planner | coder,planner / - | - / planner | - / - | planner / planner | planner / coder,planner | - / - | coder,planner / - | planner / planner | coder,planner,reviewer,sassy / planner | coder,planner / coder,planner,sassy | coder,planner / - |
| Reviewer:12 | social | jonathan,planner / jonathan,planner | jonathan,planner / jonathan,planner | jonathan,planner / jonathan,planner | - / planner | - / planner | planner / planner | - / - | jonathan,planner / jonathan,planner | - / - | jonathan,planner / jonathan,planner | jonathan,planner / planner | - / - |
| Sassy:5 | social | coder,jonathan,planner / planner | coder,jonathan,planner / jonathan,planner | - / planner | - / planner | - / - | coder,jonathan,planner / - | planner / - | jonathan,planner / coder,jonathan | planner / planner | jonathan,planner / jonathan,planner | planner / coder,jonathan,planner,reviewer | - / - |
| Coder:13 | social | jonathan,planner,reviewer,sassy / jonathan,planner,reviewer,sassy | planner / jonathan,planner,reviewer,sassy | jonathan,planner,sassy / - | jonathan,planner,sassy / - | jonathan,planner,reviewer,sassy / -… | jonathan,reviewer,sassy / jonathan,sassy | - / - | planner / jonathan,planner,reviewer,sassy | jonathan,planner,reviewer,sassy / jonathan,planner,sassy | jonathan,planner,sassy / jonathan,planner | jonathan,planner,sassy / jonathan,planner,sassy | - / jonathan,sassy |
| Planner:7 | work | w / coder,reviewer | w / w | w / w | w / w | coder,reviewer / w | w / w | w / w | w / w | w / w | w / w | w / w | w / w |
| Jonathan:focus | handover | coder,planner,reviewer,sassy / coder,planner,reviewer,sassy | coder,planner,sassy / planner | planner / coder,planner,sassy | planner / coder,planner | planner / planner | coder,planner,sassy / coder,planner,sassy | coder,planner / planner | coder,planner,reviewer,sassy / coder,planner,sassy | coder,planner / planner | coder,planner / coder,planner,reviewer,sassy | coder,planner,sassy / coder,planner,sassy | - / planner |
