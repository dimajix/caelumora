# Code-Review — `AgentActor` und Umfeld (Turn, Notes, Memory, State)

Befund vom 2026-09-30, auf `main` bei `c1eee1e`. Die daraus abgeleiteten Issues sind [151](../issues/151-agent-refactoring-epic.md) (Epic) und 152–160; N3 ist das bestehende Issue [92](../issues/92-ledger-for-notes-and-memory.md).

## Auftrag und Vorgehen

Ziel ist Vereinfachung, in vier Richtungen, die sich teils widersprechen:

| Kürzel | Ziel |
|---|---|
| **A** | Komplexe Logik vereinfachen |
| **B** | Anzahl der Hilfstypen reduzieren |
| **C** | Gesamtmenge Code reduzieren |
| **D** | Funktionalität in eigene Klassen auslagern |

Gelesen wurden vollständig: `actor_impl/agent.rs`, `agent_turn.rs`,
`tool_calls.rs`, `agent_notes.rs`, `agent_memory.rs`, `agent_state.rs`,
`tool.rs` (der `GuardedToolExecutor`), `context.rs` (Runtime) sowie im
Modell `agent.rs`, `chain.rs`, `transcript.rs`, `history`,
`message.rs`, `note.rs`, `memory.rs`, `context.rs`; dazu die Komponenten,
die den `Turn` benutzen (`notes`, `private_notes`, `agent_memory`,
`create_plan_tool`, `stop_tool`, `state_context_provider`,
`channel_history`, die beiden Appender) und die Stellen im `ChannelLoop`,
die den Actor steuern. Aufruferzahlen sind per `grep` über die drei
Workspace-Crates gezählt, **ohne Testmodule**. Das Review vom 2026-09-28
(`2026-09-28-code-review.md`) und die daraus umgesetzten Issues 83, 84, 85
sind der Ausgangspunkt: was dort erledigt ist, wird nicht wiederholt, und
die Entscheidungen von dort (z. B. `ToolCalls` als eigene Sicht, Issue 83
Nachtrag) werden nicht neu aufgemacht.

Umfang (Zeilen Produktionscode / Testcode):

| Datei | prod | test |
|---|---:|---:|
| `runtime/actor_impl/agent.rs` | 1112 | 246 |
| `runtime/agent_turn.rs` | 643 | 424 |
| `runtime/tool_calls.rs` | 124 | 44 |
| `runtime/agent_notes.rs` | 138 | 0 |
| `runtime/agent_memory.rs` | 183 | 0 |
| `runtime/agent_state.rs` | 144 | 74 |
| `model/agent.rs` | 265 | 39 |
| `model/context.rs` | 335 | 158 |
| `model/chain.rs` | 241 | 161 |
| `model/transcript.rs` | 158 | 96 |
| `model/message_history.rs` | 117 | 20 |
| `model/note.rs` | 145 | 75 |
| `model/memory.rs` | 163 | 28 |
| **Summe** | **3768** | **1365** |

Jeder Befund hat eine Nummer (A = Actor, T = Turn, N = Notes/Memory,
M = Modell, H = Hygiene), den Vorschlag, die Kosten und die Ziele, auf die
er einzahlt oder gegen die er läuft. Abschnitt 7 fasst das in einer
Matrix zusammen und schlägt eine Reihenfolge vor.

---

## 1. Gesamtbild

Die Struktur ist nach den Issues 83–85 richtig geschnitten: das Modell hält
Fakten (`AgentChannelState`, `TurnState`, die Ketten), `Turn` ist der
Handle, den Actor und Komponenten benutzen, die Fachlogik von Notes und
Memory sitzt in Wrappern, der Actor behält I/O. Die verbleibende
Komplexität liegt nicht im Schnitt, sondern an vier Stellen:

1. **Der Run-Loop hat drei voneinander unabhängige Signale für „Turn
   beenden"** (`processing_state` mit CAS, `turn_cancel`-Token, das
   `stopping`-Flag) und dazu eine handgeschriebene Vorrangregel
   (`agent.rs:1024`). Das ist die eine echte Komplexität im Actor (A1).
2. **Das Ende eines Turns ist dreimal geschrieben** — `fail_turn`,
   `give_up_lowered`, der Stop-Zweig in `run` — mit zwei Hilfs-Enums
   (`TurnFlow`, `CarriedCalls`), die dieselbe Information zweimal
   tragen (A4).
3. **`handle_llm_turn` mischt vier Dinge**: Iterationslimit,
   Plan-Modus-Klassifizierung, Streaming mit Abbruch, Plan-Fortschritt —
   117 Zeilen mit dreimaligem Stempeln derselben Felder (A2).
4. **Notes und Memory berechnen „aktueller Stand plus dieser Turn" von
   Hand**, in acht kleinen `matches!`-Scans über die staged changes, statt
   die Faltung wiederzuverwenden, die `NoteChain::current` schon hat (N1);
   und `NoteChain`/`MemoryChain` sind nach wie vor derselbe Typ zweimal
   (Issue 92, N3).

Das Modell selbst — `Chain<E>` als Baum, `ContextRef`/`ContextResolver`,
`TranscriptView`, `AgentStateHandle` — ist in Ordnung und wird nur an den
Rändern angefasst (tote Oberfläche, M1).

Grobes Ergebnis, wenn alles Empfohlene umgesetzt wird: rund 300–350 Zeilen
Produktionscode weniger, netto drei Hilfstypen weniger (siehe Abschnitt 7).

---

## 2. Der Actor (`caelumora-runtime/src/actor_impl/agent.rs`)

### A1 — Drei Signale und eine Vorrangregel für das Ende eines Turns

`cancel()` (`agent.rs:1085–1102`) schaltet `processing_state` per
CAS-Schleife (`ProcessingStateHolder::transition`) auf `Cancelling` und
löst dann das Token aus. `stop()` (`1104–1110`) setzt `stopping` und löst
das Token aus. Der Run-Loop prüft danach beides getrennt: `stopping` bei
`1008`, und bei `1021–1027` die Regel „wenn der Zustand inzwischen
`Cancelling` ist und der aktuelle Schritt nicht `Cancelling` war, dann
ignoriere, was der Handler zurückgibt". Dazu kommt `wakeup()`, das den
Zustand von außen ändert (`Sleeping → Idle`). `processing_state` hat damit
drei Schreiber (Loop, `cancel`, `wakeup`) und der Loop muss den
Zwischenschreiber erraten.

**Vorschlag:** Ein Signal statt drei. Ein `enum Interrupt { Cancel, Stop }`
in einem `AtomicU8` (oder `std::sync::Mutex<Option<Interrupt>>`) auf dem
Actor. `cancel()` setzt `Cancel` (wenn ein Turn läuft) und löst das Token
aus; `stop()` setzt `Stop` und löst das Token aus; `stop` gewinnt über
`cancel`. Der Loop nimmt nach jedem Schritt das Signal heraus
(`take_interrupt()`) und entscheidet allein: `Cancel` → nächster Zustand
`Cancelling`; `Stop` → `give_up_turn(Stopped)`, speichern, `break`. Damit

- schreibt nur noch der Loop `processing_state` (plus `wakeup`, siehe
  unten) — die Vorrangregel bei `1024` und das `transition` in `cancel`
  entfallen;
- `stopping: AtomicBool` entfällt;
- `handle_waiting_for_approval` (`604–607`) bleibt wie es ist: es kehrt
  beim Token zurück, und der Loop findet das Signal.

`wakeup` bleibt ein Zustandswechsel (`Sleeping → Idle`), weil der Moderator
und `ChannelLoop::wakeup` den Zustand *lesen* (`actor.state() ==
Sleeping`). `transition` hat dann genau diesen einen Aufrufer und kann ein
`compare_exchange` werden (siehe M3).

`Cancelling` als gespeicherter `ActorState` bleibt: ein Absturz zwischen
Cancel und Reflect nimmt dann wie heute mit „Turn beenden" wieder auf. Ihn
ganz zu streichen (Interrupt → direkt `reflect`) wäre ein weiterer Schritt,
bei dem ein solcher Absturz den Turn in `Planning` fortsetzen würde; nicht
empfohlen.

**Kosten:** ~40 Zeilen berührt, ein Enum mehr (gegen **B**), zwei Felder
und eine Regel weniger. Tests in `channel.rs` (Cancel/Stop während Approval,
Lowering) decken das Verhalten ab. **Ziele:** A stark, C leicht.

### A2 — `handle_llm_turn` (`agent.rs:710–827`)

Vier Dinge in einer Funktion:

- **Stempeln** von `sender_name`, `sender_id`, `label`, `internal` steht
  zweimal (`757–760`, `766–769`), und `message.internal = internal_message`
  ein drittes Mal bei `812`, wo es bereits gesetzt ist (H1).
- **Plan-Fortschritt** (`774–806`): drei Arten, den Plan zu befragen
  (`has_plan`, `plan()`, `take_plan`), ein `unreachable!`, und zwei feste
  Events (`CompleteMessageGenerated` bei `781`, `IntermediateMessageGenerated`
  bei `790`), obwohl `event_for(msg, false)` beides aus `message.internal`
  ableiten würde — `internal` ist genau dann gesetzt, wenn der Plan noch
  nicht finalisiert.
- Die Abbruchlogik im Stream (`745–752`) ist richtig und bleibt.

**Vorschlag:**

1. `fn stamp(&self, message: &mut Message, turn, complete: bool)` für
   Name, Id, Label und `internal`; `internal` aus `turn.plan_progress()`
   abgeleitet.
2. `Turn::plan_progress() -> Option<PlanProgress>` ersetzt `has_plan` und
   das Muster `plan().map(|p| p.progress)` (dreimal in `agent.rs`).
3. Die Plan-Buchhaltung nach einer Antwort ohne Tool-Calls auf den Turn:
   `Turn::plan_converged(&mut self) -> Vec<ActorEvent>` — bei
   `Step(idx)`: `PlanStepFinished` plus das Ergebnis von
   `advance_plan(idx + 1)`; bei `Finalizing`: `take_plan()`, keine Events;
   ohne Plan: leer. Der Actor sendet dann die Antwort mit `event_for`, die
   Events der Reihe nach, und kehrt bei laufendem Plan mit `Planning`
   zurück. Der `match` mit `unreachable!` verschwindet.

**Kosten:** Actor −35 Zeilen, Turn +15; `take_plan` behält der
`create_plan`-Tool-Provider. **Ziele:** A, C; leicht D (Logik vom Actor
auf den Turn, wie 83 es wollte).

### A3 — `request_approvals` baut `ToolCallState`s auf dem Actor (`agent.rs:635–694`)

Der Actor kennt das Layout von `ToolCallState` (Struct-Literal, dann
`state.status = Approved; state.result = Some(..)`), und er kennt die Regel
„Aufrufe, die ohnehin scheitern, sofort beantworten" (verboten → Forbidden,
unbekannt/ungültig → `to_result`, sonst beschreiben). Dieses Wissen liegt
sonst im `GuardedToolExecutor` (`tool.rs`: `check`, `failed`, `describe`,
`is_permitted`).

**Vorschlag:** `GuardedToolExecutor::admit(&self, call, turn) ->
Admission` mit

```rust
enum Admission {
    /// Requires a user's decision; the description shown to them.
    Ask { description: String },
    /// May run as it is.
    Run,
    /// Cannot run; the model is answered at once.
    Answered(ToolCallResult),
}
```

und `ToolCallState::of(call, admission)` (oder zwei Konstruktoren
`pending`/`answered`) im Modell. `request_approvals` wird zu ~12 Zeilen:
Aufrufe → `admit` → `ToolCallState` → `register` → Requests senden. Die
Logik, welches Tool `requires_approval` hat, wandert damit ebenfalls in den
Executor, der die Tools des Turns ohnehin bekommt.

**Kosten:** Actor −45, `tool.rs` +30, Modell +10; ein Enum mehr (gegen
**B**). **Ziele:** D, A.

### A4 — Drei Give-up-Pfade, zwei Hilfs-Enums für eine Information

`fail_turn` (`484–495`), `give_up_lowered` (`529–538`) und der Stop-Zweig
in `run` (`1008–1014`) tun dasselbe: Grund bestimmen, `carried_calls`,
`give_up_turn`, `TurnFailed` senden (außer bei Stop), Fehler beim Senden
warnen. Dazu:

- `TurnFlow { Flows, Lowered(c), NoChannel }` (`893–899`) und
  `CarriedCalls { InFull, NamesOnly { classification } }`
  (`agent_turn.rs:90–98`) — `carried_calls` (`518–524`) übersetzt das eine
  in das andere, und `GivenUpReason::Lowered { channel }` trägt die
  Klassifikation ein drittes Mal.
- `give_up_lowered` setzt `NamesOnly` hart und ruft `carried_calls` nicht.

**Vorschlag:**

1. `CarriedCalls` streichen: `give_up_turn(reason, names_only:
   Option<Classification>)` — `None` = vollständig, `Some(c)` = nur die
   Namen, klassifiziert `c`. `carried_calls` liefert dieses `Option`.
2. Ein `async fn give_up(&self, turn, reason, told: Option<String>,
   output)`: loggt, ruft `carried_calls`, `give_up_turn`, sendet
   `TurnFailed { message }` wenn `told` gesetzt ist. `fail_turn` wird ein
   Aufruf davon, `give_up_lowered` auch, der Stop-Zweig auch (`told:
   None`).
3. `TurnFlow` bleibt dreiwertig — `NoChannel` ist ehrlich („kann die
   Policy nicht fragen"), und `run` braucht die Unterscheidung. Wer es
   weiter reduzieren will: `NoChannel` wie `Flows` behandeln und beim
   Tragen konservativ bleiben (`Some(turn.classification())`).

**Kosten:** −1 Typ, ~30 Zeilen weniger. Mit A1 zusammen, weil der
Stop-Zweig sich ändert. **Ziele:** B, C, A.

### A5 — `AgentActorState` ist ein Mutex um zwei Felder, von denen eines konstant ist

`AgentActorState { agent_channel_state, agent_state: Arc<AgentStateHandle> }`
(`72–78`); `agent_state` wird nach `new` nie ersetzt und in `run` nur
geklont (`956`).

**Vorschlag:** `agent_state: Arc<AgentStateHandle>` als Feld des Actors,
`state: Mutex<AgentChannelState>`; die Destrukturierung bei `951` entfällt.
**Kosten:** −1 Typ, −10 Zeilen. **Ziele:** B, C.

### A6 — `loop_count`/`max_iterations` sind Turn-Arbeitszustand auf dem Actor

Zwei Atomics (`68–69`), gesetzt in `request_response` (`218`), gelesen in
`handle_llm_turn` (`717–721`). Sie gehören zum Turn: ein nach einem Absturz
wieder aufgenommener Turn zählt heute bei 0 weiter, was das Limit von 25
Modellaufrufen unterläuft.

**Vorschlag:** `TurnState::model_calls: u32` (im Modell, gespeichert),
`Turn::count_model_call() -> bool` (ob das Limit erreicht ist), `const
MAX_MODEL_CALLS: u32 = 25` in `agent_turn.rs`. Das Speicherformat ändert
sich (serde `default`, kein Altbestand zu schützen).
**Kosten:** −2 Felder, ±0 Zeilen. **Ziele:** A (Konsistenz beim Resume),
leicht B.

### A7 — `message_context` und `given_up_turns` haben denselben Vorspann

`1058–1083`: zweimal Attribute laden, `ReadAgentContext` verlangen, Zustand
laden, Fehler mappen. `GivenUpTurnsError { Forbidden, Failed }`
(`actor.rs:95–101`) ist eine Teilmenge von `MessageContextError`.

**Vorschlag:** `async fn readable_state(&self, caller) ->
Result<AgentChannelState, ReadStateError>` mit `ReadStateError {
Forbidden, Failed }`; `given_up_turns` gibt diesen Fehler zurück
(`GivenUpTurnsError` wird zu ihm umbenannt), `message_context` mappt ihn
mit einem `From` auf seinen eigenen. REST-Mapping bleibt (`context_forbidden`).
**Kosten:** −15 Zeilen, kein Typ mehr oder weniger (Umbenennung).
**Ziele:** C.

### A8 — Kleinere Doppelungen im Actor

- „Zustand setzen, dann speichern" steht dreimal (`971–972`, `1011–1012`,
  `1021–1028`): `async fn settle(&self, turn, next: ActorState)`.
- `handle_cancelling` und `handle_reflecting` (`568–582`, `877–888`) sind
  bis auf `requested_state` gleich: ein `handle_ending(turn, output,
  cancelled)`.
- `kind: String` ist konstant `"agent"` (schon im Review vom 28.09.
  genannt, 3.1.10); `id` und `agent_id` sind dasselbe (`id =
  ActorId::Agent(agent_id)`).
- `AgentActor::next_turn` (`200–206`) heißt wie `Turn::next_turn`, tut aber
  etwas anderes (Kontext bauen): `assemble_context`.
- `begin_turn` (`333–344`) hat einen Aufrufer (`request_response`).
- `store_channel_state` (`283–292`) hat zwei Doc-Kommentare übereinander.

**Kosten:** ~25 Zeilen weniger. **Ziele:** C.

### A9 — `ActionType::Publish` wird an fünf Stellen gleich gebaut

`agent.rs:506` (`turn_flow`), `tool.rs:314` (`admitted`), `user.rs:107`,
`channel.rs:347` (`reclassify`) und `channel.rs:847` (das Gate für jede
Nachricht): jedes Mal `Publish { classification, actor, channel }` plus
`security_manager.check`. AGENTS.md: eine reine Zugriffsfrage ist eine
Methode von `AccessQuestions`.

**Vorschlag:** `AccessQuestions::may_publish(&self, context, actor,
classification, channel) -> bool` neben `may_channel_read` und
`may_call_tool`; die fünf Stellen rufen sie. **Kosten:** +8 Zeilen Modell,
−10 in der Runtime; über den Auftrag hinaus, aber billig und ein Beispiel
für die Hausregel. **Ziele:** A, C.

### A10 — Die Datei nach den Schritten des Zyklus aufteilen

Auch nach A1–A8 bleiben ~950 Zeilen in einer Datei. Der Actor hat vier
klar getrennte Rollen: der Loop mit den Wartezuständen und
`cancel`/`stop`/`wakeup`; der Modellschritt (`Planning`: `call_llm`,
`label`/`stamp`, `respond_*`, `event_for`, `handle_llm_turn`); der
Tool-Schritt (`request_approvals`, `execute_tools`,
`handle_executing_tools`, `handle_waiting_for_approval`); das Ende
(`reflect`, `give_up`, `turn_flow`, `carried_calls`) und das Lesen von
außen (`message_context`, `given_up_turns`, `context_of`).

**Vorschlag:** `actor_impl/agent/{mod.rs, run.rs, model_step.rs,
tool_step.rs, ending.rs}` mit `impl AgentActor`-Blöcken — **keine neuen
Typen**, nur Dateien. Das erfüllt D, ohne gegen B zu laufen.
**Kosten:** reine Bewegung, ±0 Zeilen; erst nach A1–A4, sonst wird zweimal
bewegt. **Ziele:** D.

---

## 3. Der Turn (`agent_turn.rs`, `tool_calls.rs`)

### T1 — `EndedTurn` und `CommittedTurn`

`EndedTurn` (`53–72`) hält drei Werte, um genau ein Event zu senden;
`CommittedTurn` (`76–85`) packt ihn mit `final_messages` und
`channel_state_changed` zusammen. Das ist die Entscheidung (B) aus Issue 85
und bewusst so; die einzige Alternative mit weniger Typen wäre, `EndedTurn`
um `final_messages: Vec<Message>` und `channel_state_changed: bool` zu
erweitern und `give_up_turn` beides leer zurückgeben zu lassen — ein Typ
weniger, aber ein Typ mit Feldern, die die Hälfte der Aufrufer nicht
meinen. **Nicht empfohlen**; nur der Vollständigkeit halber.

### T2 — Die Texte für getragene Tool-Calls (`agent_turn.rs:100–135`)

Vier freie Funktionen (`executed_calls`, `given_up_because`,
`executed_in_full`, `executed_by_name`), von denen die letzten beiden zu
80 % denselben Text haben. `executed_calls` — „welche Calls dieser
Nachrichten sind gelaufen" — ist eine Frage an die Nachrichten, die auch
REST und die Web-UI (Issue 146) stellen werden, wenn sie einen
aufgegebenen Turn zeigen.

**Vorschlag:** `executed_calls` ins Modell, als
`GivenUpTurn::executed_calls(&self)` bzw. als freie Funktion über
`&[Message]` in `transcript.rs`; die beiden Texte zu einem
`fn carried_calls_message(reason, executed, in_full: bool) -> String`
(die Zeile „Their arguments and results cannot be shown here …" nur bei
`!in_full`). Die Texte bleiben Runtime (Issue 57).
**Kosten:** −15 Zeilen. **Ziele:** C, D.

### T3 — Tote und schiefe Oberfläche von `Turn`

Aufrufer im Produktionscode (ohne `agent_turn.rs`):

| Methode | Aufrufer | Vorschlag |
|---|---:|---|
| `transcript()` | 0 | streichen (Tests nutzen `agent_channel_state().transcript`) |
| `channel_id()` | 1 | streichen, `channel().channel_id` |
| `session_id()` | 4 | streichen, `channel().session_id` |
| `event_context()` | 1 (`state_span`) | bleibt, `pub(crate)` ist es schon |
| `messages()` | 1 (`commit_turn` intern) | `pub(crate)`, oder Iterator statt `Vec<Message>`-Klon |
| `staged_agent_changes()` → `&[_]`, `staged_channel_changes()` → `&mut Vec<_>` | 2 / 3 | mit N1 wird `&mut` nicht mehr gebraucht; dann beide `&[_]` |
| `set_processing_state` | 1 | bleibt (der Actor schreibt durch den Turn, weil der Turn den State geliehen hat) |

**Kosten:** −5 Methoden, ~20 Zeilen. **Ziele:** C.

### T4 — „Zwischen Turns liest alles als leer" (`NO_TURN`, `current_mut`)

`static NO_TURN`, `current()`/`current_mut()` mit `debug_assert!` und
`get_or_insert_with` (`48`, `183–193`, `437–438`) sind die Konvention aus
Issue 85 (C): der `Turn` existiert auch, wenn kein Turn läuft, weil
`perceive` durch dieselben `MessageProcessor`s geht. Sauberer wäre: ein
`Turn` existiert nur während eines Turns (`Turn::resume(state) -> Option`),
und die `PerceiveInProcessor`s bekommen `&mut AgentChannelState` statt
`&mut Turn` — sie rufen nur `extend_transcript`/`extend_history`. Dann
sind `current()` unfehlbar, `NO_TURN` und beide `debug_assert!`s weg.

**Kosten:** der `MessageProcessor`-Trait spaltet sich (Perceive auf dem
State, Emit auf dem Turn), und ~25 Tests bauen heute `Turn::new` +
`begin`. **Ziele:** A (der Typ sagt „ein Turn läuft"), gegen C. Als
Option genannt, für jetzt **nicht empfohlen** — die Konvention tut nicht
weh, solange nur zwei Appender zwischen Turns laufen.

### T5 — `ToolCalls` leiht mehr als es braucht

`ToolCalls::new(turn, agent_id, agent_name)` (`tool_calls.rs:20–29`)
braucht Id und Name nur in `collect`, um die Tool-Nachricht zu stempeln.
Gäbe `collect` die `Vec<ToolCallResult>` zurück und baute `Turn` die
Nachricht (er hat Id, Name und Klassifikation), leiht die Sicht nur
`&mut Vec<ToolCallState>` plus die Klassifikation. Das ist die kleinere
Variante der Sicht, die 83 wollte; die Entscheidung für die Sicht selbst
steht nicht zur Debatte. **Kosten:** ~10 Zeilen verschoben. **Ziele:**
leicht A.

---

## 4. Notes und Memory (`agent_notes.rs`, `agent_memory.rs`, `agent_state.rs`)

### N1 — „Aktueller Stand plus dieser Turn" wird acht Mal von Hand gesucht

`AgentNotes::note` sucht `noted_now` (`75–80`), `revoke` sucht eine
staged Note zum Entfernen und `revoked_now` (`94–100`), `reclassify`
sucht `noted_now`, `revoked_now` und dann in `current()` (`117–126`).
`AgentMemory::forget` sucht `remembered_now` und `forgotten`
(`136–139`), `reclassify` dazu `reclassified_now` (`161–169`). Jede Suche
ist ein `matches!`-Scan über die staged changes, und jede Operation stellt
dieselbe Frage: *wie sieht die Kette aus, wenn die Änderungen dieses
Turns schon drin wären?*

Genau diese Frage beantwortet `NoteChain::current` (`note.rs:109–129`)
schon — als Faltung über `NoteChange`s. Nur ist die Faltung an die
gespeicherten Einträge gebunden.

**Vorschlag:**

1. Im Modell die Faltung von der Kette lösen: `NoteChange::apply_to(&self,
   notes: &mut Vec<Note>, after, written_at)` (bzw. eine freie `fold`), so
   dass `NoteChain::current` sie über die Einträge laufen lässt und ein
   Wrapper sie über staged changes laufen lassen kann. Dasselbe für
   `MemoryChange`.
2. `AgentNotes::effective(turn, kind) -> Vec<Note>` = `current()` +
   Faltung über die staged changes des Turns. `revoke` wird: „`id` in
   `effective`? → `Revoke` stagen, sonst `false`". `reclassify` wird:
   „`id` in `effective`? → deren Klassifikation ist `current`". `note`
   wird: `max(effective ids, chain.next_id()) + 1` — oder einfacher:
   `next_id` über alle Einträge plus die staged Notes.
3. Den Sonderfall „eine in diesem Turn notierte Note wird nicht revoziert,
   sondern aus den staged changes entfernt" (`95–98`) aufgeben: ein
   `Revoke` auf eine im selben Turn notierte Note ist harmlos, die Nummer
   ist ohnehin verbraucht (`next_id` schaut über alle Einträge). Damit
   braucht niemand mehr `staged_channel_changes()` als `&mut Vec` (T3).
4. `AgentMemory::effective(turn)` analog über `recalled` + staged.

**Kosten:** `agent_notes.rs` 138 → ~90, `agent_memory.rs` −25, Modell
+15. Setzt N3 nicht voraus, wird mit N3 aber zur *selben* Faltung für
beide. **Ziele:** A stark, C.

### N2 — `forget`/`reclassify` fragen die Policy für alle Einträge, um einen zu finden

`AgentMemory::forget` (`133–145`) und `reclassify` (`158–182`) rufen
`recalled(...)` — den vollen Durchlauf mit den beiden Memo-Vektoren
`decided` und `sessions` (`41–104`) — um festzustellen, ob *ein* Eintrag
hier abrufbar ist.

**Vorschlag:** `AgentMemory::find(turn, id, reader, security, context) ->
Option<Memory>`: den Eintrag in `memory.current()` suchen, dann
`may_recall` für genau diesen. `recalled` behält die Memos (dort lohnen
sie sich, einmal pro Prompt). **Kosten:** +10 Zeilen, aber ein
Policy-Aufruf statt n. **Ziele:** A.

### N3 — Issue 92: `NoteChain` und `MemoryChain` sind derselbe Typ

Unverändert seit dem 28.09.: `note.rs:76–144` und `memory.rs:94–162`
haben `new`, `digest`, `entries`, `push`, `current` (dieselbe Faltung mit
drei Armen), `next_id` (dieselbe Suche über `all_entries`); die Enums
`NoteChange`/`MemoryChange` sind bis auf `session` gleich. Es ist die
größte einzelne Doppelung im gelesenen Code.

**Vorschlag:** wie in Issue 92, konkretisiert:

```rust
pub trait Change { type Id; type Item; fn id(&self) -> Self::Id; ... }
pub struct Ledger<C: Change, X> { changes: Chain<LedgerEntry<C, X>> }
pub struct LedgerEntry<C, X> { change: C, extra: X, after: MessageId, written_at, digest }
pub type NoteChain = Ledger<NoteChange, NoteExtra>;     // NoteExtra { transcript: Option<Digest> }
pub type MemoryChain = Ledger<MemoryChange, ChannelRef>;
```

`push` nimmt eine Digest-Funktion (die Schemata `note_chain`/`memory_chain`
bleiben), `current` faltet über `C::apply_to`, `next_id` sucht über
`C::added_id()`. Die drei Varianten (`Note`/`Remember`, `Revoke`/`Forget`,
`Reclassify`) können in einem gemeinsamen Enum `Change<Id>` aufgehen, wenn
`Remember`s Extra-Felder (`classification`, `session`) als Payload
generisch werden — oder die Enums bleiben getrennt und implementieren nur
den Trait. Letzteres ist kleiner und reicht.

**Kosten:** Modell −100 Zeilen, `agent_notes.rs`/`agent_memory.rs`
Variantennamen anpassen, Speicherformat ändert sich. **Ziele:** C stark,
und der erste Schritt von 65. Mit N1 zusammen planen.

### N4 — `NoteKind` und die zwei Varianten von `AgentChannelChange`

`NoteKind::chain/change/of` (`agent_notes.rs:30–53`) existieren, weil
`AgentChannelChange` zwei Varianten mit demselben Payload hat
(`Note(NoteChange)`, `PrivateNote(NoteChange)`). Mit `AgentChannelChange::Note
{ kind: NoteKind, change: NoteChange }` fallen `change` und `of` weg
(`apply` im Modell matcht dann auf `kind`). **Kosten:** −10 Zeilen,
Speicherformat ändert sich. **Ziele:** C. Klein; wenn 65 näher rückt,
lieber dort.

### N5 — In Ordnung

`AgentStateHandle` und `AgentStates` (`agent_state.rs`): ein Zweck, ein
Lock, ein Snapshot, klar begründet (Issue 62). `AgentState::apply`,
`AgentChannelState::apply`: je ein Aufrufer, aber die Naht für 65 — bleiben.

---

## 5. Das Modell

### M1 — Tote Oberfläche von `Transcript` und `History`

| | Aufrufer (prod) | Vorschlag |
|---|---:|---|
| `Transcript::last_message_id` | 0 | streichen |
| `Transcript::extend -> Vec<Option<Digest>>` | 2, beide ignorieren den Rückgabewert | `-> ()` (der Test nutzt `push`) |
| `MessageHistory::entries` | 0 | streichen (`all_entries` bleibt: 2) |
| `Transcript::new`, `MessageHistory::new`, `NoteChain::new`, `MemoryChain::new` | Tests | `Default` genügt |
| `Chain::children`, `leaf_from` | 0 | bleiben — für 08, Schritt 2 gebaut und getestet |

**Kosten:** −20 Zeilen. **Ziele:** C.

### M2 — `TurnEntry` trägt einen `TranscriptEntry` mit `given_up: None`

`TurnEntry { entry: TranscriptEntry, context }` (`agent.rs:203–206`);
`TurnState::push` baut den `TranscriptEntry` mit `given_up: None`, und
`record_turn` (`agent_turn.rs:550–563`) ruft dann `transcript.push(message)`,
das den Digest **neu berechnet**, und prüft hinterher, ob er dem gespeicherten
entspricht. Die Verschachtelung existiert, damit `TranscriptView::prefix`
`&TranscriptEntry` herausgeben kann (`context.rs:254–261`).

**Option:** `Transcript::push_entry(entry: TranscriptEntry)`, das den
Digest übernimmt, statt ihn neu zu berechnen; die Warnung bei `561` wird
dann zur Prüfung `entry.parent == head`. Kleiner Gewinn, gleiche
Verschachtelung. **Nicht dringend.**

### M3 — `ProcessingStateHolder` (`model/actor.rs:290–360`)

`from_raw`/`raw` mit Lücken (1, 4 = entfernte Zustände) und ein
`transition` mit CAS-Schleife, das nach A1 nur noch `wakeup` benutzt.
**Vorschlag:** `#[repr(u8)]` auf `ActorState`, `raw = self as u8`,
`from_raw` als `match` ohne Lücken; `transition` durch
`compare_exchange(Sleeping, Idle)` ersetzen (`wakeup` braucht nichts
anderes). **Kosten:** −20 Zeilen. **Ziele:** C.

### M4 — `AgentActor::is_turn_state` gehört auf `ActorState`

Vier Aufrufer, alle im Actor (`313`, `944`, `968`, `998`); die Frage „ist
das ein Zustand innerhalb eines Turns" ist eine Eigenschaft des Enums.
`ActorState::in_turn(self) -> bool`. **Kosten:** ±0. **Ziele:** —
(Sauberkeit).

### M5 — In Ordnung

`Chain<E>` (Baum, Validierung beim Laden), `ContextRef`/`ContextResolver`/
`TranscriptView`, `ModelProfile`, `ReasoningHistory`: klar geschnitten,
gut getestet, nichts zu vereinfachen ohne Verlust.

---

## 6. Hygiene (H1)

Ohne Strukturänderung, in einem Zug:

- `agent.rs:812`: `message.internal = internal_message;` ist bereits bei
  `769` gesetzt — streichen.
- `agent.rs:283–286`: zwei Doc-Kommentare auf `store_channel_state`.
- `model/agent.rs:125–129` (`TurnState`-Doc): nennt `abandon_turn`; heißt
  `give_up_turn`.
- `model/agent.rs:103–106`: „Full message history" / „Possibly preprocessed
  history used as context" beschreiben Transcript und History nicht mehr
  (siehe Doc in `transcript.rs:69–79`).
- `agent.rs:579`: „finished cancelling" — mit A8 verschwindet die Funktion.
- `agent_turn.rs:1–25` (Modul-Doc) nennt `EndedTurn` als Rückgabe beider
  Enden; nach A4 unverändert richtig, nach T1 (falls) nicht.

---

## 7. Zielkonflikte und Empfehlung

| Nr. | Maßnahme | A | B | C | D | Kosten | Empfehlung |
|---|---|:-:|:-:|:-:|:-:|---|---|
| H1 | Hygiene | | | + | | trivial | ja |
| M1, T3, A8 | tote Oberfläche, Doppelungen | | | ++ | | klein | ja |
| A1 | ein `Interrupt` statt drei Signale | +++ | − | + | | mittel | **ja** — die eine echte Komplexität |
| A4 | Give-up vereinheitlichen, `CarriedCalls` weg | + | + | + | | klein | ja, mit A1 |
| T2 | Carried-Calls-Texte, `executed_calls` ins Modell | | | + | + | klein | ja |
| A2 | Modellschritt entflechten, `plan_converged` | ++ | | + | + | mittel | ja |
| A3 | `GuardedToolExecutor::admit` | + | − | + | ++ | mittel | ja |
| N3 | Issue 92: `Ledger` | | + | +++ | | mittel | **ja** — größte Doppelung |
| N1 | `effective` statt acht Scans | +++ | | ++ | | mittel | **ja**, nach N3 |
| N2 | `AgentMemory::find` | + | | | | klein | ja |
| A5, A6, A7 | `AgentActorState`, Zähler in `TurnState`, `readable_state` | + | ++ | + | | klein | ja |
| M3, M4 | Holder verkleinern, `in_turn` | | | + | | klein | ja, nach A1 |
| A9 | `may_publish` | + | | + | | klein | ja (über den Auftrag hinaus) |
| N4 | `AgentChannelChange::Note { kind }` | | | + | | klein | eher mit 65 |
| T5 | schmalere `ToolCalls`-Leihe | + | | | | klein | optional |
| A10 | Datei nach Schritten aufteilen | | | | +++ | Bewegung | optional, nach A1–A4 |
| T4 | `Turn` nur während eines Turns | ++ | | −− | | groß | nein, für jetzt |
| T1 | `EndedTurn`/`CommittedTurn` zusammenlegen | − | + | | | klein | nein |
| M2 | `push_entry` statt Neuberechnung | | | | | klein | nicht dringend |

**Wo die Ziele sich widersprechen:**

- **A1 und A3 fügen je ein Enum hinzu (gegen B),** entfernen aber mehr
  als sie kosten: A1 zwei Felder, eine CAS-Schleife und eine Vorrangregel;
  A3 das Wissen über `ToolCallState` im Actor. Beide sind Fälle von „der
  Typ sagt, was der Code sonst über zwei Flags errät" — B ist hier das
  schwächere Ziel.
- **D gegen B:** Auslagern in *Klassen* würde Typen kosten; A10 lagert in
  *Dateien* aus und erfüllt D ohne B zu verletzen. Wo eine Klasse wirklich
  fehlt (A3, `admit`), ist es eine Methode auf einer bestehenden.
- **T4 (A) gegen C:** die sauberere Typisierung kostet einen Trait-Split
  und ~25 Tests. Nicht jetzt.
- **N3 und N1** sind zusammen mehr als einzeln: die Faltung, die N1
  braucht, ist der Kern von N3.

**Reihenfolge:**

1. H1, M1, T3, A8 — ein Commit, reine Reduktion, kein Risiko.
2. A1, dann A4 und M3/M4 — der Run-Loop; die Channel-Tests
   (`channel.rs`: Cancel/Stop während Approval, Lowering, Raising) sind das
   Netz.
3. A2, T2 — der Modellschritt.
4. A3, optional T5 — der Tool-Schritt.
5. N3 (Issue 92, mit Architekturentscheidung zum `Change`-Trait), dann N1,
   N2, N4.
6. A5, A6, A7, A9.
7. A10, wenn danach noch gewünscht.

Ergebnis in Zahlen (geschätzt, prod): Actor ~1112 → ~850, `agent_turn.rs`
~643 → ~600 (+Plan-Buchhaltung, −Texte), `agent_notes.rs` + `agent_memory.rs`
321 → ~250, Modell −100 (N3) −20 (M1) −20 (M3). Typen: −`AgentActorState`,
−`CarriedCalls`, −`GivenUpTurnsError` (umbenannt zu `ReadStateError`),
−eine der beiden Ketten (N3); +`Interrupt`, +`Admission`. Netto −3.
