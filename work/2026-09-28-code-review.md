# Strukturreview — `caelumora-model`, `caelumora-runtime`, `caelumora-rest-server`

Befund vom 2026-09-28. Die daraus abgeleiteten Issues sind [74](../issues/74-refactoring-epic.md) (Epic) und 75–111.

Stand: 2026-09-28, auf `main` bei `ed9abe8`.

## Auftrag und Vorgehen

Gefragt war ein Review mit vier Schwerpunkten: Sind die Typen sinnvoll
geschnitten? Lässt sich Code durch Vereinheitlichung vereinfachen? Können
Typen weniger öffentliche Methoden haben (Methode mit einem Aufrufer →
in den Aufrufer)? Gibt es komplexe Aufrufketten über viele Typen, die
einfacher werden können? Zusätzlich sollten die offenen Issues als
Randbedingungen einfließen, weil sie Architekturentscheidungen vorwegnehmen.

Vorgehen: Alle Quelldateien der drei Crates wurden vollständig gelesen (rund
23 000 Zeilen), aufgeteilt auf vier Durchgänge (Modell; Runtime-Kern: Runtime,
Session, Channel, Actors, Turn; Runtime-Komponenten: Komponentensystem,
Tools, Kommandos, LLM; REST-Server). Aufruferzahlen wurden per `grep` über
alle Workspace-Crates gezählt, Testmodule ausgeschlossen. Ein fünfter
Durchgang hat alle 35 offenen Issues auf strukturelle Konsequenzen
ausgewertet. Die als Fehler gemeldeten Befunde in Abschnitt 1 habe ich
anschließend selbst im Code nachgeprüft.

Die Reihenfolge innerhalb jedes Abschnitts ist nach Wert sortiert. Wo ein
offenes Issue eine Empfehlung einschränkt oder stützt, steht seine Nummer
dabei. Kosten sind grob in Aufrufstellen oder Zeilen angegeben.

---

## 1. Fehler, die dabei aufgefallen sind

Kein Strukturthema, aber vorab, weil sie Verhalten betreffen. Alle geprüft.

1. **Ein gestoppter Agent setzt beim Neustart einen halben Turn fort.**
   `AgentActor::new` übernimmt den gespeicherten `processing_state`
   (`actor_impl/agent.rs:116`), und `run` steigt bei `Planning`,
   `ExecutingTools` oder `Reflecting` sofort in `handle_llm_turn` ein — ohne
   Anlass, mit dem Arbeitszustand des abgebrochenen Turns. Das passiert nach
   `remove_participant` (Abort mitten im Turn), nach `stop_session` und nach
   einem Prozessneustart. Beim Laden gehört alles außer `Idle`, `Sleeping`
   und `WaitingForApproval` auf `Idle` normalisiert, plus `abandon_turn`.
   `WaitingForApproval` ist der einzige bewusst wieder aufnehmbare Zustand
   (Issue 04 nennt ihn als den teuersten Verlust).
   **Korrektur (Maintainer, gleicher Tag):** Das Wiederaufnehmen ist
   Absicht — ein Agent soll nach einem Server-Absturz genau dort weitermachen.
   Was bleibt: ein bewusster Stopp (`remove_participant`) ist von einem
   Absturz nicht zu unterscheiden, und `ExecutingTools` führt beim
   Wiederaufnehmen Tools erneut aus. So in Issue 75 gefasst.
2. **Ein Abbruch trennt die Transcripts der anderen Agenten von der History.**
   Solange `ChannelLoop::processing_state == Cancelling`
   (`channel.rs:656–661`), wird *jede* interne Weiterleitung unterdrückt,
   auch das `FullResponseGenerated`, mit dem der abgebrochene Turn endet. Die
   History bekommt es (`append_history` läuft davor), die Transcripts der
   Mit-Agenten nicht. Siehe Abschnitt 3.1, Befund 1: das Flag kann weg.
3. **`ParticipantTemplate::instantiate` verschluckt Store-Fehler.**
   `template.rs:235`: `else if let Ok(agent) = find_agent_by_name(…)` behandelt
   `Backend`, `PermissionDenied` und `NotFound` gleich und legt dann einen
   zweiten Agenten an. Mindestens `match … Err(StoreError::NotFound{..}) =>
   create, Err(e) => return Err(e.into())`.
4. **`McpToolConfig.requires_approval` ist wirkungslos.** `discover_tools`
   setzt `requires_approval: false` fest (`component_impl/mcp_tool.rs:133`),
   der Konfigurationswert (`:56`) wird nie gelesen. Ein MCP-Tool mit Effekt
   läuft damit ohne Freigabe (relevant für Issue 20).
5. **`UserActor` beendet sich bei einem Sendefehler.** `user.rs:189–191, 254`:
   `?` in der Run-Schleife. Der Nutzer verliert seinen Actor im Channel,
   niemand startet ihn neu. `ApproverActor` loggt in derselben Lage und läuft
   weiter (`approval.rs:335–337`) — das ist die richtige Wahl für beide.
6. **`stored_channel_response` antwortet falsch statt unbekannt.**
   `routes/events.rs:133,136`: ein nicht laufender Kanal meldet
   `can_change: false`, `can_read_context: false`. Beides ist aus dem
   `Channel`-Dokument beantwortbar (Abschnitt 5.1, Befund 3).
7. **Zwei Ids pro Approval, eine davon ungenutzt.** `ApproverActor::handle_request`
   vergibt eine neue `ApprovalId` und merkt sich die alte in `_request`
   (`approval.rs:110–126`), das nie gelesen wird. Der Moderator routet
   Nutzerantworten ohnehin an den Approver — das Umschreiben schützt nichts.
8. **`create_plan`-Parsefehler haben den falschen Status.**
   `create_plan_tool.rs:116,142` melden `ExecutionFailed` („The tool failed"),
   gemeint ist `InvalidParameter` („Correct them and call the tool again",
   `tool.rs:92–95`).

---

## 2. Randbedingungen aus den offenen Issues

Was ein Refactoring **nicht** tun sollte, weil ein offenes Issue darauf baut:

| Nicht anfassen | Grund |
|---|---|
| `Recorder`-Trait, `BlackholeRecorder`, `Mutex<Box<dyn Recorder>>` im `ChannelLoop` | 05 (Postgres-Recorder), 07 (Löschungs-Audit über Recorder), 13/14/22/28 nennen ihn als Audit-Senke. Der Runtime-Durchgang schlug vor, ihn zu entfernen — **nicht** folgen. |
| `SecurityContext::delegation_ids`, `caller_id`, `SecurityContext::next` | 12 (User × Agent-Rechte) braucht genau diesen Datenpfad. Der Modell-Durchgang listet `delegation_ids` als „0 Leser" — stehen lassen, ggf. Kommentar mit Verweis auf 12. |
| `User.roles`, `Scope::GroupScope`, `ScopeTemplate::GroupScope` | 23 hält „in Role-Scope umwandeln oder entfernen" offen. Entfernen nur mit Eintrag in 23. |
| `AgentChannelChange`/`AgentStateChange` (wenige Varianten), `Turn::stage_*`/`staged_*`, `agent_channel_state()` | 65 macht daraus den generischen Mechanismus; die Staging-Methoden sind die Naht für Komponenten und für 55. Bleiben `pub`. |
| `Chain<E>` als Typ, `entries()`/`prefix()` | 08 macht die Chain zum Baum; nicht zu `Vec` reduzieren, keine neuen Slice-Leser. |
| `ActorState::Reflecting` und die Reihenfolge Store → `AgentChannelStateChanged` → `FullResponseGenerated` | 55 hängt den Reflexions-Call in diese Phase. |
| `turn_classification`/`turn_publication` als Turn-Schnappschuss | 67 (Kanal-Level ändern) setzt ihn voraus — gerade so umgesetzt (`ed9abe8`). |
| `Publication` (zwei Bools), `ChannelClassification`, `KnowledgeSource`-Varianten | 64 (Compartments) und 58/59 (Teilmengen) machen sie feiner, nicht gröber. |
| `Tool.requires_approval` und `Tool.confidentiality` getrennt | 13 (scoped Approvals) und 20/08 (Effekt-Deklaration) kommen als weitere Felder. |
| `EventContext::span_depth`, `max_route_decisions` | 45 fügt `agent_hops` hinzu und entscheidet dann. |
| REST-DTOs in `models.rs` | 42 (Export mit eigenem Format), 24 (Policy-Input als Projektion): nie die Modelltypen direkt serialisieren. Das gilt auch für `GET /templates`, das heute `SessionTemplate` roh liefert (`routes/templates.rs:8`). |
| `EntityStore::{create,update}_user` neben `UserProvider` | 25: wer den User-Record besitzt, ist offen. |
| `ComponentContext.actor_runtime: Option<…>` | 28/69: Session-Kommandos ohne Actor. |
| `SessionReader`-Trait mit einer Implementierung | 65 macht ihn generisch, 59/18 lesen darüber. |
| `CommandExecutor` als Struct vs. `ToolExecutor` als Trait | 28 begründet die Asymmetrie. Wer angleicht, schreibt 28 fort. |
| `ToolExecutor::is_permitted` vor der Freigabe **und** `execute` prüft erneut | Absicht („policy before approval"). |

Was die Issues **stützen**:

- **65 + 08 + 16:** `AgentChannelState` in Dauerzustand und Turn-Arbeitszustand
  trennen (Abschnitt 3.1, Befund 3 des Modells). Der Arbeitszustand wird beim
  Branch-Switch verworfen, der Komponenten-State gestaged und übernommen.
- **65:** `NoteChain`/`MemoryChain` vereinheitlichen — aber in Richtung „der
  Kern kennt die Art nicht", nicht als weiterer `enum`-Fall.
- **32 + 56 + 57:** Die `deps`-Komposition im `StateContextProvider` ist genau
  die Stelle, die 32 kritisiert; die Frage „eingebaut oder Default-Komponente"
  stellt 56. Das stützt Abschnitt 4.1, Befund 1 (Facette `ContextProvider`
  streichen).
- **23 + 24:** eine totale Abbildung `ActionType → (Kind, Verb)`; die
  `Find*`-Varianten, die die Ownership-Policy immer erlaubt, passen da nicht
  hinein und können mit dem Store-Dialekt (Abschnitt 3.2, Befund 3) gehen.
- **10:** fabrizierte `SecurityContext`s sollen eine greppbare Stelle werden
  (`SecurityContext::default()` in `tui/main.rs:403`, `rest-server/data.rs:38`,
  `routes/events.rs:156`).

Bereits verworfene Ansätze, die nicht erneut vorgeschlagen werden sollten:
Soft-Delete (07); Truncate-and-rerun statt Branch, Fork per `prev_id` (08);
`ChannelLoop::execute_command(user, …)` und Commands pro Kanal (28); Gruppen
als ACL-Subjekt, Store-Handle im `SecurityManager` (23); neue Facette oder
vierte Zyklusphase für Skills (22); zentraler Prompt-Katalog als erster
Schritt (57); `AgentChannelState` als Handle mit Snapshot/Deltas, inhaltstragende
`NotesChanged`-Events (65); teilnehmerabhängige Lesbarkeit (12/19).

---

## 3. `caelumora-model` und der Runtime-Kern

Die beiden hängen so eng zusammen, dass die Befunde zusammen stehen.

### 3.1 Schnitt der Typen

1. **Das Cancel-Flag des Channels ist ein dritter Zustandsautomat für dieselbe
   Sache.** `ChannelLoop::processing_state: Arc<AtomicU8>` (`channel.rs:74`),
   `AgentActor::processing_state` und `AgentChannelState::requested_state`.
   Das Channel-Flag hat genau eine Wirkung (Fehler 2 oben) und ist der Grund
   für `ActorState::Active` (`model/actor.rs:324–326`) und die öffentlichen
   `From<u8>`-Konvertierungen. **Vorschlag:** Flag streichen, `cancel` ruft
   nur `actor.cancel()`; `Active` aus `ActorState`. Kosten: `channel.rs` 5
   Stellen, `agent.rs` 2, Modell 1.

2. **`AgentActor` führt den Turn, `Turn` ist nur eine Sicht.** Jede
   `Turn`-Methode ist `self.agent_channel_state.<feld>`; die Logik —
   `execute_tools` (`agent.rs:328–412`), `request_approvals` (`785–839`),
   `advance_plan` (`463–488`), `plan_step_message`/`plan_finalize_message`
   (`414–453`) — steht im Actor und braucht von ihm nur `tool_executor`,
   `agent_id` und den Namen. Das ist der in AGENTS.md beschriebene Fall „ein
   Objekt lässt ein anderes für sich handeln": der Actor ruft 8–10
   Turn-Setter pro Schritt. **Vorschlag:** Was nur Turn-Zustand liest und
   schreibt, auf `Turn` (`advance_plan`, `collect_tool_results(...) ->
   Option<Message>`, `register_tool_calls(states)`); der Actor behält I/O
   (LLM, Executor, `output`). Kosten: ~250 Zeilen bewegen, keine externen
   Aufrufer. Nebenbefund: `AgentActorState::turn()` holt den Namen aus
   `turn_event_context.sender_name` (`agent.rs:68–75`); die Quelle ist
   `agent_controller.name()`.

3. **`AgentChannelState` vermischt Dauer- und Turn-Zustand.** Von 19 Feldern
   (`model/agent.rs:175–247`) sind 11 Arbeitszustand eines Turns (`turn_*` ×8,
   `tool_calls`, `plan`, `requested_state`), die `next_turn`, `commit_turn`
   und `abandon_turn` in drei sich überlappenden Listen von Hand zurücksetzen
   (`agent_turn.rs:124–129, 133–138, 152–158`). **Vorschlag:** `pub turn:
   Option<TurnState>` mit Sub-Struct; Beginn = `Some(TurnState::new(…))`,
   Ende = `None`. „Kein Turn läuft" wird zum Typ statt zur Konvention über
   elf Felder; Fehler 1 oben wird beim Laden zu `turn = None`. Gestützt von
   65 und 08 (Arbeitszustand wird beim Switch verworfen). Kosten: ~60
   Zugriffe, alle in `agent_turn.rs`. Bis dahin: `next_turn` =
   `abandon_turn()` + `requested_state = None` + die vier fixierten Werte,
   eine Liste statt drei.

4. **Die fünf Controller sind Orchestrierung im Modell.** `SessionController`,
   `ChannelController`, `AgentController`, `UserController`,
   `ApproverController` halten Live-Zustand (`ArcSwap`), schreiben in den
   Store, veröffentlichen `GlobalEvent`s und serialisieren mit
   `tokio::sync::Mutex` — nur ihretwegen hängt das Modell von `tokio/sync` ab.
   Dazu fünf Bauarten: `SessionController::new` ist `async` und `Result` ohne
   `await`/Fehler (`session.rs:31–44`); `ChannelController.entity_store` ist
   `Option`, `None` nur in Tests; `AgentController` ohne `write_lock`;
   `UserController` ohne Store, `update_username` ohne Aufrufer;
   `SessionController::process_global_event` besteht zu 6 von 7 Zweigen aus
   `// TODO` (`session.rs:129–155`), und `SessionLoop::run` abonniert das
   Session-Topic nur dafür. `ApproverController` ist ein Wrapper ohne Zweck.
   **Vorschlag:** nach `caelumora-runtime/src/controller.rs` verschieben (dort
   ist bereits die Fabrik), eine Form (`new(entity, publisher, store)` sync,
   `write_lock` überall, ein `store_and_announce`-Helfer), TODO-Zweige und
   `ApproverController` streichen. Kosten: ~14 `use`-Zeilen, sonst keine
   Logikänderung.

5. **Persistierte Entitäten tragen einen `SecurityContext`.**
   `Agent::security_context` (`model/agent.rs:147`), `User::security`
   (`user.rs:16`). Beide Actors überschreiben ihn beim Start
   (`agent.rs:84–85`, `user.rs:46–47`) — der Controller hält danach eine
   kanalspezifische Kopie eines gespeicherten Objekts. Ein Kontext ist etwas,
   das ein Actor *hat*. **Vorschlag:** Felder streichen; der Actor baut
   `SecurityContext::new(ActorId::…, session, channel)` aus dem
   `ActorRuntime`, das beide Ids kennt. Kein offenes Issue deckt diese
   Felder. Kosten: 2 Runtime-Stellen + Fixtures.

6. **Ein Atomic im gespeicherten Dokument.**
   `AgentChannelState::processing_state: Arc<ProcessingStateHolder>` mit
   eigenem serde-Modul (`model/actor.rs:439–465`); der Actor klont den `Arc`
   (`agent.rs:116`). **Vorschlag:** im Zustand ein plain `ActorState`, der
   Actor besitzt den Holder und schreibt den Wert beim Speichern hinein;
   `processing_state_serde` und `default_processing_state` entfallen.
   Zusammen mit Befund 3 und Fehler 1 in einem Zug.

7. **Template-Instanziierung macht Store-IO im Modell.**
   `ParticipantTemplate::instantiate` (`template.rs:210–253`) liest User,
   sucht und legt Agenten an; `SessionContext` transportiert dafür
   `Arc<dyn EntityStore>`. Beide Frontends bauen `SessionContext` + Mapping +
   `instantiate` von Hand (`rest-server/routes/sessions.rs:13–53`,
   `tui/main.rs:406–414`). **Vorschlag:** das Modell liefert aus dem Template
   die *Anforderung* (Name, Scope, PersonalityId), ein
   `SessionManager::create_from_template(ctx, template_id, name)` löst sie
   auf; `SessionContext` entfällt, Fehler 3 damit auch. Nebenbefund: der Store
   hat kein `get_session_template(id)`, weshalb der REST-Handler alle
   Templates listet und filtert (`sessions.rs:20–24`).

8. **`ChannelRuntime` ist eine Delegator-Schicht ohne Eigenleben.**
   `runtime.rs:95–105` sind sechs Pass-throughs an `RuntimeManager`;
   `with_session` (`77–80`) wird an beiden Stellen unmittelbar nach `new`
   gerufen. **Vorschlag:** `pub(crate) fn manager()` statt sechs Delegatoren,
   `new(manager, commands, session)` statt Builder. `ActorRuntime` bleibt
   bewusst schmal (69) — dort ist die Delegation gewollt. Kosten: 2
   Produktions- und 8 Testaufrufer.

9. **Doppelte Actor-Tabelle im Channel.** `ChannelLoop::actors:
   ArcSwap<HashMap<…>>` (`channel.rs:65`) und in `run` eine lokale
   `HashMap<ActorId, ActorHandle>` (Sender, AbortHandle, `:498`), synchron
   gehalten über `changes_tx`/`ActorChange` mit Sonderfall „joined before run
   started" (`529–533`). Läge der Handle im ArcSwap-Eintrag, entfielen Queue
   und `apply_change`; das Ende-Kriterium „letzter Actor weg" (`513–517`)
   müsste ersetzt werden — praktisch endet ein Channel heute nur mit der
   Session. Mittlerer Aufwand; zuerst nur den doppelten Aufnahmeblock
   (`105–116` und `245–252`) zu `fn adopt(&self, actor)` ziehen.

10. **Kleinere Duplikate.** `ChannelLoop` hält `id`, `session_id` neben
    `channel.load()` (`58–61`); `SessionLoop::id()` liest den Controller, das
    Feld `self.id` wird daneben benutzt; `AgentActorState.agent_id` dupliziert
    `AgentActor.agent_id`, `kind: String` ist konstant `"agent"`;
    `ApproverActor` hält einen eigenen `Arc<ChannelController>` nur für
    `get_user_count` (`approval.rs:36, 51–54`).

**Fein so:** `Chain<E>` mit `Transcript`/`MessageHistory`; `confidentiality.rs`;
`AccessQuestions`; `store_impl/access.rs`; `mention.rs`; `digest.rs`;
`AgentChannelState::resolve`/`apply`; `SessionManager` (Start-Lock,
`running`/`may`); `AgentStates`/`AgentStateHandle`; `AgentNotes`/`AgentMemory`
als Wrapper über dem generischen `Turn`; `SessionReader` mit `NotReadable`;
`EventBus` mit Dispatcher für `subscribe_as`; die Moderatoren.

### 3.2 Vereinheitlichung

1. **`NoteChain` und `MemoryChain` sind derselbe Typ.** `note.rs:68–122`,
   `memory.rs:74–132`: gleiche Methoden, gleiche `current()`-Faltung, gleiche
   `next_id()`-Suche; Unterschied nur die Zusatzfelder (`transcript` vs.
   `origin`) und die Digest-Funktion. **Vorschlag:** ein generischer
   `Ledger<X>` (Eintrag = `Change { Add{id,text}, Remove{id} }` + Extra `X` +
   `after` + `written_at` + Digest); `NoteChain = Ledger<NoteExtra>`,
   `MemoryChain = Ledger<ChannelClassification>`. Das ist der erste Schritt
   von 65 — richtig gemacht, wenn der Kern die Art nicht kennt. Kosten: ~100
   Zeilen weniger im Modell, Variantennamen in `agent_notes.rs`/`agent_memory.rs`,
   gespeicherte Form ändert sich (kein Altbestand).

2. **Drei Actor-Run-Loops, drei Fehlerstile** (`agent.rs:1077–1146`,
   `user.rs:234–264`, `approval.rs:310–356`). Alle: `select!` über `rx` +
   `subscribe`, gleiche Log-Zeilen, Behandlung von `None`; nur die
   Fehlerbehandlung weicht ab (Fehler 5). **Vorschlag:** für die zustandslosen
   Actors ein gemeinsamer Rahmen `run_reactive(rx, tx, on_envelope,
   on_global)`, überall „loggen, weiterlaufen". Der Agent behält seine
   Zustandsmaschine. Kosten: 2 Dateien, ~60 Zeilen.

3. **Der `EntityStore` hat drei CRUD-Dialekte.** `store.rs:31–77`:
   Personalities/Templates/Components `put`+`delete`+`list`; Agents
   `create`/`update` ohne `list`/`delete`; Sessions/Channels
   `create`/`update`/`delete`; Users `create`/`update`/`find` ohne
   `list`/`delete`. Session-Templates werden per **Name** abgelegt, per **Id**
   gelöscht (Vollscan, `json.rs:366–378`). `find_*_by_name` dreimal in zwei
   Bauweisen (`memory.rs:101–110` vs. `195–206`). **Vorschlag:** ein Dialekt
   für alle sieben Kinds — `create/get/update/delete/list`, alle per Id —
   und `find_by_name` für alle oder keinen. Die `Find*`-`ActionType`s
   entfallen (passt zu 23/24). Für 03 (Postgres) ist ein einheitlicher
   Dialekt die Voraussetzung, dass ein Implementierungs-Testsatz alle drei
   Backends gleich prüft. Kosten: 2 Implementierungen × ~7 Methoden, ~10
   Aufrufer.

4. **User werden im Store nicht geprüft.** `memory.rs:293–332`,
   `json.rs:535–585`: `// Placeholder for security check`, obwohl
   `ActionType::GetUser` und `SecurityAttributes::owned_by(User)` existieren.
   **Vorschlag:** `User: Secured` und dieselben `Access`-Aufrufe. 25 lässt
   offen, wer den Record besitzt, aber nicht, ob er geprüft wird.

5. **Vier Arten, den `SecurityManager` zu fragen.** `SessionManager::may`
   (`session.rs:470–476`, Fehler `StoreError`), `ChannelLoop::check`
   (`channel.rs:381–387`, `ChannelError::Forbidden`), inline in
   `AgentActor::message_context` (`agent.rs:1171–1174`) und `publication`
   (`317–326`); Log-Texte und -Level unterscheiden sich. **Vorschlag:** neben
   `AccessQuestions` ein `require(ctx, action, refusal: E) -> Result<(), E>`
   mit einem `info!`-Muster.

6. **`handle_idle`/`handle_sleep`** (`agent.rs:559–603`, `666–692`) sind
   strukturgleich bis auf „startet `ResponseRequested` einen Turn". Ein
   `handle_waiting(state)` mit `match (state, event)` spart ~35 Zeilen und
   `state_after_sleep`. Ebenso `respond_partial`/`respond_complete`
   (`166–198`) und die viermal wiederholte Wahl `internal → Intermediate…,
   sonst Complete…` (`903–907, 926/936, 958–962, 989–993`) → `fn
   event_for(message, partial)`.

7. **`ActorState::Stopping` tut nichts.** `handle_stopping` (`agent.rs:710–713`)
   loggt und leitet nach `Reflecting`; die Doku darüber („fall through to
   Idle") stimmt nicht. `handle_executing_tools` (`1014–1021`) liest
   `requested_state()` nur, um `Stopping` zu erkennen. **Vorschlag:**
   `Stopping` streichen; `requested_state == Some(_)` heißt „nach diesem
   Tool-Schritt reflektieren". Kosten: Modell 3, `agent.rs` 3, `stop_tool` 1.

8. **Fehlertypen für „nicht erlaubt"/„nicht gefunden".** `StoreError::PermissionDenied`
   (unit), `SecretError::PermissionDenied(String)`, `ChannelError::Forbidden{…}`;
   `StoreError::NotFound{entity,id}` vs. `SecretError::NotFound(String)`.
   `ChannelError` wird nur vom Runtime konstruiert — gehört dorthin.

9. **Konfiguration.** `PersistenceBackend` ist `#[serde(untagged)]` mit
   `Memory{_dummy}` (`config.rs:34–41`), alle anderen Sektionen sind
   `tag="type"`; `create_entity_store`/`create_state_store` (`220–248`) sind
   identische Matches, die dreimal `MemoryStore` bauen; `persistence`/
   `secrets`/`users` sind `Option`, `auth`/`logging`/`commands`/`security`
   haben `#[serde(default)]`. **Vorschlag:** `enum PersistenceBackend {
   #[default] Memory, Json { path } }` mit `tag="type"`, alle Sektionen
   `#[serde(default)]`, ein `store(backend)`-Helfer. Kosten: Beispiel-Configs,
   3 Tests.

10. **Kleinigkeiten mit gleichem Muster.** Sechs Default-Funktionen für
    `Confidentiality::Public` (`channel.rs:235`, `template.rs:80/84`,
    `agent.rs:122`, `message.rs:98/102`, `tool.rs:101`) → `impl Default` +
    `is_public()`; `ToolId` und `SessionTemplateId` handgeschrieben statt
    `define_id!`; `ActorId::Approver(Uuid)` statt `ApproverId` (→ `approver.id.0`
    an 4 Stellen); `Envelope::forward_with_response` ≡
    `forward_with_event(FullResponseGenerated(m))`; JSON-Store:
    `read_json_opt::<Persisted<T>>(…)?.map(|p| p.value)` 24-mal, `Persisted {
    schema_version: 1, … }` 18-mal, `schema_version` wird nie gelesen;
    `create_*` prüft `path.exists()` synchron zwischen `tokio::fs`-Aufrufen;
    `get_session_state` liefert im JSON-Store `Default`, im Memory-Store
    `NotFound`, und `SessionState` wird nirgends gelesen oder geschrieben —
    Typ und vier Store-Methoden können weg. `ModeratorActors` nutzt `DashMap`
    hinter `&mut self`; `add_actor` sortiert per Downcast statt über
    `ActorId`; `FilteringEventSender::send` verwirft still, `try_send` gibt
    `Err(Filtered)` — eines von beiden.

### 3.3 Öffentliche Schnittstelle

Aufrufer = Nicht-Test-Aufrufer außerhalb der definierenden Datei.

**Ohne Aufrufer (entfernen, sofern kein Issue sie hält):**

| Item | Vorschlag |
|---|---|
| `Envelope::{forward, respond, with_event, with_security}` (`actor.rs:232–274`), `ActorEvent::from_message_event` (`:158`), `ActorId::as_user_id` (`:46`) | entfernen |
| `ChannelLoop::{actor_names, actor_by_name}` (`channel.rs:389, 456`) | entfernen |
| `ChannelLoop::actor_ids` (`:392`) — 1 Aufrufer (REST sucht danach per `actor_by_id`) | entfernen, REST nutzt `members()` |
| `EventBus::{subscribe_all, subscribe_agent, subscribe_user}` (`event.rs:198, 210, 214`), `EventSubscription::try_recv` | entfernen bzw. `#[cfg(test)]` |
| `InMemoryEventReceiver::prepend`, `EventReceiver::{drain_matching, take_buffered}` (`transport.rs:31–35`) | aus dem Trait entfernen |
| `UserController::update_username`, `ApproverController`, `format_conversation` (`message.rs:149`), `Digest::as_bytes`, `ToolId`, `STAGE_PLAN`/`STAGE_EMIT` | entfernen |
| `AgentChannelState::seen_context_of` (`agent.rs:334`) — der Runtime ruft `recorded_context_of` + `resolve` getrennt | eines von beiden |
| `SessionState` + 4 `StateStore`-Methoden | entfernen |
| `Message::metadata: HashMap` (`message.rs:44`) — kein Schreiber | entfernen |
| `SecurityContext::delegation_ids` — kein Leser | **behalten** (12) |
| `Scope::GroupScope` (einziges `todo!()` im Produktionscode, `template.rs:59`), `User::roles`, `User::email` | **nur mit Eintrag in 23** |
| `ToolParameters::boolean`, `ToolConfidentiality::CALL_ONLY` | behalten (vollständiger Konstruktor-Satz / dokumentiertes Vokabular) |

**Ein Aufrufer (inline oder enger):**

| Item | Aufrufer | Vorschlag |
|---|---|---|
| `SessionLoop::cancel_channel` (`session.rs:261`) | REST `channels.rs:362`, hat den Channel schon | entfernen; REST ruft `channel.cancel(ctx)`. Zudem `anyhow!` statt `SessionError`, Parameterreihenfolge umgekehrt |
| `SessionManager::load_session` (`:429`) | `open_session` | privat; `open_session` prüft `running`/`may` doppelt (`432`, `462`) → eine Methode |
| `ChannelLoop::channel()` (`channel.rs:170`), `wakeup_actor` (`:356`) | 1 | `pub(crate)`; `channel()` wird mit REST-Befund 5.1.3 zum einzigen Weg |
| `ChannelRuntime::{security_manager, state_store, event_bus, controller_manager, new_moderator}` | je 1–2, alle `channel.rs` | `manager()` (Befund 3.1.8) |
| `ActorRuntime::{new_tool_executor, new_agent_controller, new_user_controller, personality, llm_client, load_channel_state, store_channel_state, announce_channel_state_changed}` | je 1, alle `actor_impl` | `pub(crate)`; `channel_attributes` privat |
| `RuntimeManager::agent_states` (`runtime.rs:371`) | intern | privat; `new_moderator` `pub(crate)`; `new -> Result` ist infallibel → `Self` (6 Aufrufer verlieren `?`) |
| `Turn::{full_context, last_message, set_tool_calls, set_tool_approval_status, take_tool_calls, tool_calls, set_plan_progress, take_requested_state, next_turn, commit_turn, abandon_turn, send_event, event_context, saw_private_knowledge}` | nur `agent.rs` | `pub(crate)`. Die Komponenten-API (`set_context`, `record_knowledge`, `stage_*`, `extend_*`, `plan`/`set_plan`/`take_plan`, `request_state`, `agent_state`, `classification`, `publication`, `security_context`) bleibt `pub` (65) |
| `Turn::{has_plan, has_tool_calls, requested_state, transcript, messages, context}` | 0–2 | `has_*` inline, `requested_state` weg (Peek+Take → nur Take), `transcript` weg, `context` privat, `messages` `pub(crate)` |
| `Turn::agent_id`, `agent_name` (pub Felder) | viele | eine Konvention: Felder oder Getter |
| `Transcript::prefix` (`message.rs:255`) 0, `prefix_entries` 1 | — | `prefix` weg, `prefix_entries` → `prefix` (mit Blick auf 08: „Pfad zum Head") |
| `Channel::new` (`channel.rs:246`) | nur Fixtures, Produktion `build` | privat |
| `Actor::wakeup`, `Actor::cancel -> Result`, `async` (`actor.rs:59–65`) | `ChannelLoop` | nie fehlschlagend, nie `await`end → `fn wakeup(&self)`; `?` in `channel.rs:327, 363, 374` und der REST-Fehlerpfad `channel_wakeup_failed` entfallen |
| `SessionLoop::get_session` vs. `SessionManager::get_session` | viele | gleicher Name, anderes Ding → `SessionLoop::session()`, `SessionManager::running(id)` |
| `store.rs:103 pub mod tests` | — | `mod tests` |

### 3.4 Komplexe Pfade

**A. „Ein Nutzer schreibt, ein Agent antwortet."** REST `send_message` →
`ensure_session` → `channel_of` → `user_actor_of` → **`wake_for_delivery()`**
→ `UserActor::send_message` (zwei Envelopes in `user_tx`) → `UserActor::run`
reicht sie in den Channel-`mpsc` → `ChannelLoop::run` → `process_envelope` →
`route_event` → Moderator → `FilteringEventSender` → `AgentActor::handle_idle`:
`FullResponseGenerated` → `perceive` (**`begin_turn`**) → `ResponseRequested`
→ `request_response` (**`begin_turn` erneut**) → … → `reflect` → `commit_turn`
→ `FullResponseGenerated` → Channel → SSE. Drei Vereinfachungen:

- *`wake_for_delivery` aus REST und TUI in den Channel.* Der Channel sieht den
  Nutzer-Envelope selbst; der auskommentierte Block `channel.rs:650–654`
  wollte genau das. Zwei Frontend-Aufrufer (`channels.rs:329`,
  `tui/main.rs:304`) und ein Fehlerpfad entfallen; `send_approval` weckt heute
  *nicht* — Asymmetrie ohne Grund.
- *Die `UserActor`-Queue ist ein Hop ohne Zweck.* `user_tx/user_rx`
  existieren nur, weil der Channel-Inbox-Sender erst in `run` entsteht
  (`channel.rs:496`). Wird `event_tx` in `ChannelLoop::new` erzeugt und als
  `pub(crate) fn submit(&self, Envelope)` angeboten, sendet `UserActor`
  direkt. Kosten: `channel.rs` 2 Stellen, `user.rs` 4 Methoden + 2 Tests.
- *`begin_turn` in `perceive`* (`agent.rs:149`; von mir in `ed9abe8` so
  gebaut, um die Setter zu vereinheitlichen) fragt jetzt pro wahrgenommener
  Nachricht und Agent zweimal den `SecurityManager`, obwohl `request_response`
  gleich beides noch einmal tut. Zu prüfen, was die `PerceiveIn`-Prozessoren
  wirklich brauchen — vermutlich nur den `EventContext` für die Provenienz.
  Dann gehört `next_turn` nur in `request_response`, und `perceive` setzt
  den Kontext ohne Turn-Beginn.

**B. „Ein Tool-Call braucht Approval."** Der Agent geht `Planning →
ExecutingTools` (weil `has_tool_calls`) → `execute_tools` sieht `Pending` →
`WaitingForApproval` → `ExecutingTools`. `request_approvals` weiß bereits, ob
etwas `Pending` ist, und kann `WaitingForApproval` direkt zurückgeben; dazu
Fehler 7 (Doppel-Id) und `handle_timeouts` (`approval.rs:266–275`), das den
Envelope selbst baut statt `send_decision(p, Timeout, …)` zu rufen.

**C. „Ein Agent wird hinzugefügt"** (`channel.rs:214–255`) ist inhaltlich
richtig; kürzer wird sie durch Befund 3.1.9.

**D. „Ein Actor wird gestoppt"** → Fehler 1.

**E. „Eine Session wird geladen."** `open_session` → `running`/`may` →
`load_session` → Lock → `running`/`may` **erneut** → … Die Tiefe ist die Sache
selbst; nur die doppelte Frage entfällt mit einer Methode.

**F. Digest-Kette `Transcript::push` → `push_message` → `Chain::push_with` →
`message_chain` → `Encoder`** ist nachvollziehbar, **keine Änderung**. Aber
`Message::prev_id` wird im Digest ausgelassen, „weil die Kette es ausdrückt",
und `ollama.rs:377`/`agent.rs:397` setzen es trotzdem — entweder das Feld
oder die Setzer. (08 will den Fork-Punkt per `MessageId`, nicht `prev_id`.)

**G. Approver auf drei Wegen.** `ChannelTemplate::instantiate` erkennt den
magischen Namen `"approval"` (`template.rs:100`), `ParticipantTemplate::Approver`
existiert daneben, und `Channel::build` fügt sowieso einen hinzu
(`channel.rs:270–276`). Keine Demodaten nutzen die ersten beiden. Nur
`Channel::build` entscheidet; `ParticipantTemplate::name()` wird dann immer
`Some`.

---

## 4. `caelumora-runtime` — Komponentensystem, Tools, Kommandos, LLM

### 4.1 Schnitt der Typen

Insgesamt tragfähig: Facetten liegen neben ihrem Nutzer (`tool.rs`,
`command.rs`, `context.rs`, `message.rs`), jede Komponente hat eine Datei mit
`KIND`, `register` und Tests; `Turn` ist generisch, die Fachlogik sitzt in
`AgentNotes`/`AgentMemory`.

1. **`ContextProvider` ist eine Facette mit genau einer Implementierung und
   undefiniertem Verhalten bei mehreren.** `AgentActor::next_turn`
   (`agent.rs:201–214`) iteriert die Liste; jeder ruft `Turn::set_context` und
   überschreibt den vorigen. `StateContextProvider` sammelt die
   `PromptProvider` aus seinen `deps` — der bekannte Stolperstein (Kommentar
   `tui/main.rs:40`, Doku `create_plan_tool.rs:42–44`), den Issue 32 als
   „still verlierbar" kritisiert. **Vorschlag (b):** Facette streichen; der
   Actor holt `get_all::<dyn PromptProvider>` selbst und baut die
   `ContextItem`s wie heute `current_context`; Reihenfolge = Konfiguration.
   Betrifft `context.rs`, `state_context_provider.rs` (weg), die Configs in
   `tui/main.rs` und `rest-server/data.rs`, ARCHITECTURE. **Vorbehalt:** 16
   (Kompression) erzeugt einen neuen Transcript mit Herkunftsrelation — das
   ist eine Operation *auf* dem Transcript, kein zweiter Kontextlieferant,
   und braucht die Facette nicht. Sollte das anders gesehen werden, ist (a)
   die Alternative: genau einen `ContextProvider` erzwingen, sonst
   `InvalidConfig`. Architekturentscheidung — vorher abstimmen.

2. **`ComponentFactory` ist synchron, `ComponentRegistry::build` asynchron**
   (`component.rs:91` vs. `:133`). Deshalb muss `mcp_tool::register`
   `block_in_place` + `block_on` nutzen und eine Multi-Thread-Runtime
   verlangen (`mcp_tool.rs:263–278`). **Vorschlag:** Factory liefert
   `BoxFuture`; ein `register_sync`-Helfer packt die 14 synchronen ein.

3. **Tags und Stages sind tot.** `ComponentContainer::get_all(tag)` wird an
   allen 6 Stellen mit `None` gerufen; `nodes(tag)`, `ComponentNode.tags`,
   `STAGE_*` haben keinen Leser. **Vorschlag:** Parameter weg; Container als
   `Vec<ComponentNode>` in Bauordnung statt `HashMap + node_order`
   (`:183–184`); `ComponentError::NodeNotFound` kann nicht mehr eintreten,
   `get_all` wird unfehlbar (6× `?` weg).

4. **`AgentNotes`/`AgentMemory` als Structs bringen nichts.** Jede Nutzung ist
   `AgentNotes::new(turn, kind).note(...)` — konstruieren, einmal rufen,
   fallen lassen (6 Stellen); die Lesefunktionen sind schon assoziiert.
   **Vorschlag:** durchgehend assoziierte Funktionen. Die unterschiedliche
   Nummernvergabe (Notes aus Chain + Staging; Memory über
   `reserve_memory_id` auf dem geteilten Handle, weil mehrere Actors eines
   Agenten parallel schreiben) ist begründet — gehört als Absicht in
   `agent_memory.rs` (19 dokumentiert es als Ausnahme von „Commit statt
   Write").

5. **`EmitOutProcessor` hat keine Implementierung**; der Loop führt die leere
   Liste aus (`agent.rs:170–198`). Behalten und in ARCHITECTURE als
   Erweiterungspunkt nennen — oder streichen. `MessageProcessor::name` hat
   null Aufrufer.

### 4.2 Vereinheitlichung

1. **`DefaultToolExecutor` nach dem Muster von `CommandExecutor`.** Der
   Executor scannt pro Aufruf alle Provider und ruft `provider.tools()`
   (`tool.rs:224–226, 258–260`; bei MCP jedes Mal `cached_tools.clone()`);
   `request_approvals` (`agent.rs:791–808`) tut es zweimal mehr;
   `AgentActor::label` pro Stream-Chunk (`agent.rs:271, 900`). `new` iteriert
   die Tools bereits für die Dublettenprüfung. **Vorschlag:** `entries:
   Vec<ToolEntry { tool, component, provider }>` wie `CommandEntry`. Nur
   `tool.rs`. Damit sind die acht defensiven `_ => Err(UnknownTool)`-Arme in
   den Providern nachweislich unerreichbar.

2. **Das Tool-Ergebnis hat drei Repräsentationen.** `execute` liefert
   `ToolCallResult`, der Loop nimmt nur `.content` (`agent.rs:351`), legt es
   mit dem Status getrennt in `ToolCallState { result: Option<String>,
   result_status }` ab und baut daraus per Hand wieder ein `ToolCallResult`
   (`agent.rs:381–390`, mit `c.result.unwrap()`). **Vorschlag:**
   `ToolCallState.result: Option<ToolCallResult>` im Modell — Status wandert
   mit, `unwrap` und `result_status` entfallen. Das passt zu 20, das
   `ToolCallState` einen Lebenszyklus geben will. Alternative mit weniger
   Umbau: `ToolExecutor::execute` liefert `(ToolResultStatus, String)` und
   bildet `ToolError` selbst ab (dann muss der Loop `available` nicht kennen).

3. **Drei „etwas in eine Chain schreiben"-Komponenten.** `notes`,
   `private_notes`, `agent_memory` sind strukturgleich (zwei Tools, ein
   `BeforeTurnInput`-Teil, ein `KnowledgeRef`). Ein generischer
   `ChainComponent<K>` lohnt sich **nicht** — die Fremdteile (`read_notes`,
   fremde Notizen, „elsewhere") dominieren, und 65 will die Generik im Kern,
   nicht in der Komponente. Wohl aber gemeinsame Helfer: `note_id`
   (`notes.rs:163`) ist in `agent_memory.rs:93–96` kopiert → `id_param<T:
   From<u32>>`; `note_text` wird aus `notes` importiert, gehört nach `tool.rs`;
   drei `render`-Funktionen → `render_sections(preamble, sections)`;
   `read_error` (`notes.rs:172–177`) ist in `channel_history.rs:68–71`
   dupliziert → `NotReadable::to_tool_error` in `session_reader.rs`.

4. **Parameter parsen: vier Stile.** `params.get("x").and_then(Value::as_str)`,
   `params["expression"].as_str()`, serde-Struct, Durchreichen. **Vorschlag:**
   serde-Structs mit `from_value(...).map_err(InvalidParameter)` überall, ~8
   Stellen. Dabei Fehler 8 beheben.

5. **Namen und Konstanten.** Kind-Konstanten teils `*_TOOL_PROVIDER_KIND`,
   teils `*_KIND`; die Kommando-Kinds liegen im Modell, alle anderen im
   Runtime-Modul; Struct-Namen `NotesComponent` vs. `ChannelHistoryReader` vs.
   `CreatePlanToolProvider`; Tool-Namen als Konstante oder doppeltes Literal
   (`calculator_tool.rs:21,33`, `echo_tool.rs:20,32`, `stop_tool.rs:26–51`);
   `facets` vs. `capabilities` als Variablenname (4 Dateien); Konstruktion
   `::new(reader)` vs. Struct-Literal. Alles mechanisch. Nach dem Rename
   context→turn (`f5dcaff`) heißen Parameter noch `context: &Turn` in
   `notes.rs:124,191`, `private_notes.rs:122`, `agent_memory.rs:92,111`,
   `calculator_tool.rs:32`, `echo_tool.rs:31`, `agent.rs:517`.

### 4.3 Öffentliche Schnittstelle

| Item | Aufrufer | Vorschlag |
|---|---|---|
| `ToolExecutor::providers` (`tool.rs:153`) | 0 | entfernen |
| `ToolExecution`, `ExecutionStatus` (`tool.rs:125–142`) | 0 (lokales `_execution`, nie gelesen) | entfernen; `execute` schrumpft ~15 Zeilen |
| `MessageProcessor::name`, `ComponentContainer::empty`, `OllamaClient::with_think` | 0 | entfernen |
| `OllamaClient::from_env` | nur `Default` | privat |
| `LLMError::describe_chain` (`llm.rs:31`) | nur `ollama.rs` | private fn dort; `redact_endpoint` bleibt pub (AGENTS-Regel) |
| `unsuccessful_result`, `successful_result`, `denied_result`, `timed_out_result`, `forbidden_result`, `ToolError::to_result` | 0–1 | privat / `pub(crate)`; mit 4.2.2 wandern sie in den Executor |
| `parse_plan`, `PLANNING_SYSTEM_PROMPT` (`create_plan_tool.rs`) | in-Datei | privat (57 macht den Prompt später zum Komponenten-Parameter) |
| `McpError`, `McpResult`, `McpAuthConfig`, `McpToolConfig` | in-Datei | `pub(crate)` |
| `NotesComponent::new`, `PrivateNotesComponent::new`, pub Felder von `StateContextProvider`/`StaticPromptProvider` | register + Tests | `pub(crate)` |
| `AgentMemory::{recalled, digest}`, `AgentNotes::digest`, `CommandExecutor::empty`, `ComponentContext::new`, `ComponentContainer::{from_config, get_all}` | Komponenten / Runtime | behalten |

### 4.4 Komplexe Pfade

- **Kontext bauen:** `AgentActor::next_turn` → je `ContextProvider` → je
  `PromptProvider::get_prompt` → `record_knowledge` → Sortierung →
  `ContextItem`s → `set_context` → später `full_context` → LLM; `commit_turn`
  leitet `prompt_refs` aus denselben Items ab. Der Kern ist sauber (eine
  Quelle für „gesendet" und „aufgezeichnet"); der Umweg ist die Facette
  (4.1.1), ohne sie zwei Ebenen weniger.
- **Tool-Aufruf:** drei Scans, zwei Security-Checks (gewollt), drei
  Ergebnisformen. Mit 4.2.1 und 4.2.2: ein Scan, ein Typ, `agent.rs:350–357`
  wird ein Match-Arm.
- **MCP:** `block_on` in der Factory (4.1.2), `strip_prefix` baut
  `format!("{}__")` pro Aufruf (`mcp_tool.rs:78–85`), Fehler 4.
- **`create_plan` genehmigen:** zweimaliges Parsen ist bei `describe ->
  String` unvermeidlich; in Ordnung bis auf Fehler 8.

---

## 5. `caelumora-rest-server`

Vorbemerkung: `caelumora-client-api`, `caelumora-rest-client`,
`caelumora-local-client` haben je eine leere `lib.rs`. Der einzige Client ist
die Web-UI.

### 5.1 Schnitt der Module

1. **`data.rs` (304 Zeilen, 11 % der Crate) gehört nicht in den Server — und
   existiert zweimal.** `setup_initial_data` legt Benutzer, Personalities und
   Templates an; `tui/main.rs:95–150` tut dasselbe mit *anderen* Prompts. Die
   `--user`-Doku (`main.rs:30–31`, „Owner of the templates") ist falsch:
   gespeichert wird mit `SecurityContext::system()`. **Vorschlag:** eine
   Seed-Stelle (Runtime oder Seed-Datei, die `AppConfig` benennt), aus beiden
   Binaries entfernen; zugleich eine der drei Stellen aus Issue 10. Getrennt
   planen, weil es die TUI mitzieht.

2. **Response-Konstruktion über drei Module verstreut.** `channel_response`
   in `routes/channels.rs:117`, `stored_channel_response` und
   `session_snapshot`/`describe_participant`/`describe_session` in
   `routes/events.rs:71–140`, `knowledge_response` in `channels.rs:456`.
   Deshalb ist `routes::channels` als einziges Untermodul `pub`, und
   `sessions.rs:100` importiert seine Antwortform aus dem Events-Modul.
   `models.rs` hat mit `From<PublishedNotes>`/`From<SeenItem>` das Muster
   bereits. **Vorschlag:** alle nach `models.rs`, Signatur `(&dyn
   SecurityManager, viewer, …)` statt `&AppState`. Reine Verschiebung.

3. **`channel_response` und `stored_channel_response` sind eine Funktion über
   `Channel`.** `can_read_context` ist `may_read_agent_context(viewer,
   &agent.security_attributes)`, aus `Participant::Agent` beantwortbar;
   `can_change` ist `may_change_channel`; `handle` wird zweimal identisch
   berechnet; `kind` liefert der Actor als Literal `"agent"`. **Vorschlag:**
   `ChannelResponse::describe(&Channel, security, viewer)`; laufend über
   `ChannelLoop::channel()`. Behebt Fehler 6.

4. **`AppState`.** `state_store` ungenutzt (Compiler-Warnung); `entity_store`
   ist dasselbe Handle wie `runtime_manager.entity_store()`. **Vorschlag:**
   `AppState { auth, session_manager, runtime_manager }`.

5. **`auth.rs`** vereint DTOs, Handler, Middleware und Router — vertretbar,
   in sich geschlossen, bestgetestet. Lassen.

**Fein so:** `error.rs` (Problem Details, Extractor-Wrapper, Request-Id als
`task_local`), `sse.rs` (Token-Bindung, Lag-Resync), `routes/mod.rs`.

### 5.2 Vereinheitlichung

1. **Session/Kanal-Lookup: dasselbe Paar 12-mal.** `ensure_session` +
   `channel_of` in `channels.rs` (103/105, 137/138, 154/155, 201/202, 274/276,
   290/292, 325/327, 356/360, 385/386, 400/401, 436/437, 494/496), sechs davon
   dann `user_actor_of`; `user.security_context()` bis dreimal pro Handler.
   **Vorschlag, Runtime-Seite (besser):** `SessionLoop::channel_for(ctx, id)
   -> Result<Arc<ChannelLoop>>` als Gegenstück zu `open_session`, das
   `NotFound` liefert, wenn `may_read_channel` verneint; `readable_channels(ctx)`
   für `list_channels` und `describe_session`. Dann stellt der Server die
   Zugangsfrage nirgends selbst. Server-Seite: `open_channel(state, user, id,
   cid)` und `acting_user(…)`. ~30 Zeilen Runtime, ~40 weniger im Server.

2. **Sechs Fehler-Mapper mit einem Muster.** `session_error`, `channel_error`,
   `participant_error`, `cancel_error`, `command_error`,
   `message_context_error`, plus ein Inline-Mapper in `get_session`
   (`sessions.rs:92–99`). Alle: downcasten → Status, sonst Fallback;
   `command_error:418` matcht `CommandError::Forbidden`, das `is_forbidden`
   eine Zeile später ohnehin abdeckt. **Vorschlag:** *ein* `api_error(error,
   code_prefix, action)` in Tabellenform (Forbidden-Familie → 403, NotFound →
   404, `AlreadyParticipant`/`DuplicateChannelName` → 409, sonstige
   Domänenfehler → 400, sonst `internal`). Achtung: die UI matcht auf
   `context_forbidden` (`ContextPanel.tsx:26`) — Codes bleiben aus der
   Variante ableitbar. ~80 → ~35 Zeilen.

3. **Zwei identische SSE-Aufbauten.** `global_events` (`events.rs:17–43`) und
   `join_channel` (`channels.rs:192–257`): subscribe → Snapshot → Resync →
   `resync_on_lag` → `chain` → `bounded_by_token`; `snapshot_event` zweimal
   identisch, `to_string(..).unwrap_or_default()` dreimal. **Vorschlag:**
   `sse::stream(user, options, receiver, what, snapshot)` und
   `sse::named_event(name, &impl Serialize)`.

4. **`SessionResponse` 4-mal von Hand, `name` doppelt** (`name:
   meta.name.clone(), metadata: meta`; die UI liest nur `id, name`). `impl
   From<&Session>`, `metadata` streichen. Der Zweig „laufende Session
   bevorzugen" in `list_sessions` (`sessions.rs:64–69`) ist tot: Umbenennen
   speichert zuerst, Store und Live-Objekt sind identisch.

5. **Viewer aus der Session: drei Schreibweisen** (`initiator_id`,
   `viewer_id(&user)`, `ActorId::User(uid) == me`) →
   `AuthenticatedUser::actor_id()`. **UUID → `ActorId` zweimal
   unterschiedlich** (`channels.rs:293` vs. `438–440`) → ein Weg.

### 5.3 Öffentliche Schnittstelle

In einer Binary-Crate meldet der Compiler `pub`-Items als tot; der Wert
engerer Sichtbarkeit ist dokumentarisch.

| Item | Aufrufer | Vorschlag |
|---|---|---|
| `ApiError::status` (`error.rs:104`) | nur Tests | `#[cfg(test)]`, Warnung weg |
| `AppState.state_store` | 0 | streichen |
| `TokenService::lifetime_secs`, `SESSION_COOKIE`, `SESSION_EXPIRES_COOKIE`, `REQUEST_ID_HEADER` | nur eigene Datei | privat |
| `Auth.tokens`, `Auth.users` (pub Felder) | Konstruktion in `main.rs` | `Auth::new`, Felder privat |
| `viewer_id`, `may_read_channel` (`mod.rs:58`) | 1 / 3 | entfallen mit 5.2.5 / 5.2.1 |
| `channel_response` pub, `pub mod channels` | 5 | nach `models.rs`, Modul privat |
| Cargo: `dashmap`, `tracing-subscriber` | 0 Verwendungen in `src` | aus `Cargo.toml` |

### 5.4 Komplexe Pfade

1. **Kanal-Stream beitreten** (`channels.rs:192–257`, 9 Schritte): auth →
   `ensure_session` → `channel_of` → `channel.subscribe()` →
   `event_bus().subscribe_channel(cid)` → Snapshot → Resync über `Weak` →
   `resync_on_lag` → `take_until(Mitgliedschaftsschleife)`, die
   `UserChannelLeft` filtert und bei Lag `actor_by_id(me)` prüft →
   `bounded_by_token`. Die Mitgliedschaftsschleife ist Runtime-Wissen im
   Server — 25 Zeilen subtiler Glue plus `event_bus` und `Weak` im Handler.
   Issue 73 hat entschieden, dass der Kanal keine eigene Leave-Notification
   hat; die Kosten liegen jetzt sichtbar hier. **Vorschlag:**
   `ChannelLoop::subscribe_as(ctx) -> Result<broadcast::Receiver<Envelope>>`,
   das die Leseprüfung macht und schließt, wenn der Nutzer den Kanal verlässt
   (Sender pro `UserActor`, beim Stop gedroppt). Handler: `open_channel` →
   `subscribe_as` → `sse::stream(…)`, ~12 Zeilen. Das ist kein Widerspruch zu
   73 — kein zweiter Broadcast, sondern das natürliche Ende eines
   Abonnements — aber 73 sollte fortgeschrieben werden.
2. **Nachricht senden** → Abschnitt 3.4 A (Wecken in den Channel).
3. **Teilnehmer hinzufügen** — sauber, nur Lookup-Paar und Mapper.
4. **Authentifizieren** (`auth.rs:254–296`) — klar strukturiert;
   `session_cookies` sollte `(cookies, expires_at)` zurückgeben statt dass der
   Aufrufer `expires_at` neu berechnet (`:267`); `kid` ist der *Name* des
   Secrets (`:91`) — ein Kommentar fehlt.
5. **Sessions listen** — der Live-Zweig ist redundant (5.2.4). Der globale
   Snapshot (`events.rs:48–95`) macht pro Session × Kanal einen Policy-Call
   und pro Mitglied einen weiteren — heute unproblematisch, der Ort, an dem
   `readable_channels(ctx)` auch Leistung brächte.

### 5.5 Sonstiges

- **Cancel ist eine zweite Zugangsentscheidung im Server.** `cancel_channel`
  (`channels.rs:361`) verlangt via `user_actor_of` Teilnehmerschaft, bevor
  die Runtime `CancelChannel` prüft. Nach AGENTS.md gehört „nur Teilnehmer
  dürfen abbrechen" in die Policy — oder entfällt.
- **`GET /templates` liefert `SessionTemplate` roh** inkl.
  `security_attributes` und `commands`; die UI liest `metadata.name`. Ein
  `TemplateResponse {id, name, description}` (42). `templates.rs:2` nutzt
  `axum::Json` statt `crate::error::Json` — einzige Abweichung von der
  AGENTS-Regel.
- **Veraltete Doku:** `GlobalSnapshot` „Unfiltered for now … issue 11" (ist
  gefiltert); `user_actor_of` „Until participants can be added (46)";
  `KnowledgeRefResponse.source` nennt drei Werte, der Code liefert auch
  `"memory"`.
- `auth.rs:139` `expect(...)` beim Cookie-`HeaderValue`; `unwrap_or_default()`
  bei der JSON-Serialisierung an vier Stellen sendet bei Fehler stumm ein
  leeres `data:` — mindestens `warn!`.
- `kind` als `String` mit festem Vokabular (`"user"/"agent"/"approver"`) →
  `enum` mit `rename_all`, schreibt den Vertrag zur UI fest.
- Bind-Adresse fest (`main.rs:101`, `127.0.0.1:3000`); `me` bei unbekanntem
  Nutzer löscht das Cookie nicht, `authenticate` schon.

---

## 6. Querschnitt

**`#[serde(default)]` nur für Altdaten.** Mit „kein Altbestand zu stützen"
können entfallen: alle 15 auf `AgentChannelState`, `AgentState::memory`,
`Channel::history_limits skip_serializing_if`, `ContextRef::prompts`,
`KnowledgeSource::History.start`, `ToolCallState::result_status` (Doku „stored
before it existed"), `ToolCallResult.status` samt Test
`stored_results_without_a_status_read_as_ok`, `SecurityAttributes::contributors`,
`Session::commands` samt Test „Sessions stored before…". **Bewusst behalten**
auf handgeschriebenen Dokumenten (`Personality::{components,
security_attributes, reasoning_confidentiality}`, `Channel::confidentiality`,
`ComponentDescriptor`, `ChannelTemplate`, `AppConfig`) — dort ist das
Weglassen Ergonomie, nicht Legacy, und so sollte es kommentiert sein. Ebenso
weg: Kommentare „formerly in TemplateStore" (`store.rs:33, 40`), „stored
before owners were recorded" (`security.rs:97`).

**`unwrap`/`expect`/`unsafe` außerhalb von Tests.** `agent.rs:385`
`c.result.unwrap()` (entfällt mit 4.2.2); `approval.rs:324`
`.to_std().unwrap()`; `transport_impl/memory.rs:55` `remove(i).unwrap()`;
`component.rs:215, 230, 245` (Topo-Sort); `auth.rs:139`. `unsafe` in
`memory.rs:49–50` ist vermeidbar durch `pub type EventPredicate = for<'a>
fn(&'a Envelope) -> bool` in `transport.rs`. `digest::to_json` hasht bei
Serialisierungsfehler stillschweigend `Null` — in einem Integritätsmerkmal
wäre ein begründetes `expect` ehrlicher.

**Infallible `Result`, `async` ohne `await`.** `RuntimeManager::new`,
`ChannelLoop::spawn_actors`, `handle_stopping`; `SessionLoop::channel_by_name`,
`ControllerManager::new_agent_controller`, `SessionController::new`;
`MemoryStore::read_lock`/`write_lock` geben `StoreResult`, liefern immer `Ok`
(30 `?` umsonst).

**Locks und Clones.** `ChannelLoop::state: Arc<Mutex<…>>` und
`processing_state: Arc<AtomicU8>` werden nie geklont — `Arc` weg.
`SessionLoop` hält „meine Channels" dreifach (`channels: DashMap`, `tasks:
Mutex<HashMap>`, `start_tx/rx`) → ein `DashMap<ChannelId, ChannelEntry>`.
`route_event` klont das Envelope (`channel.rs:658`), obwohl `route_event_impl`
nur liest; `record_turn` klont `turn_messages` zweimal; `replace_role` +
`remove_tools` klonen die Nachrichten zweimal; `get_user` klont im
`MemoryStore` die ganze Map.

**Logging.** `DefaultToolExecutor::execute` (`tool.rs:198–205`) schreibt fünf
Ids in die Nachricht — gehören auf den Span; `ollama.rs:407` loggt alle
Message-Ids pro Request bei `debug!` — Größe statt Liste.

**Doku, die dem Code widerspricht.** ARCHITECTURE.md:271–274 nennt
`ToolExecutionRequested/Granted/Denied`, der Code `ApprovalRequested/…`;
ARCHITECTURE nennt `ComponentNodeConfig`, der Typ heißt `ComponentDescriptor`;
`session_reader.rs:10` und `private_notes.rs:10` verweisen auf ein
`ChannelLoop::may_read`, das nicht existiert; `channel_history.rs:4` auf
`notes::SessionReader`; `agent_turn.rs:290–294`: der Doc-Kommentar zu
`record_knowledge` steht über `saw_private_knowledge`; `agent.rs:219–222`
(„checks a cancel() that raced in") — die Funktion prüft nichts;
`RouteDecision::broadcast`-Doku stimmt nicht mehr; `EventContext::span_depth`
vs. Logging-Feld `depth` (AGENTS.md). `issues/INDEX.md:143–145`: der Text von
Issue 36 steht in der Zeile von 53.

---

## 7. Empfohlene Reihenfolge

Nach Wert, mit Blick darauf, was aufeinander aufbaut:

1. **Fehler 1–8** (Abschnitt 1). Klein, unabhängig, sofort.
2. **Channel-Cancel-Flag und `ActorState::Active` streichen** (3.1.1) — behebt
   Fehler 2, entfernt einen Zustandsautomaten.
3. **Turn-Arbeitszustand als `Option<TurnState>`** (3.1.3) zusammen mit dem
   plain `ActorState` im Dokument (3.1.6) — dabei Fehler 1 strukturell
   gelöst. Vorbereitung für 65 und 08.
4. **Turn-Logik von `AgentActor` auf `Turn`** (3.1.2); `Stopping` weg (3.2.7);
   `handle_idle`/`handle_sleep` und die Event-Wahl zusammenlegen (3.2.6).
5. **Pfad A:** Wecken in den Channel, `UserActor`-Queue weg, `perceive` ohne
   Turn-Beginn (3.4 A); REST und TUI werden kürzer.
6. **`DefaultToolExecutor` als Tabelle, ein Ergebnistyp, `ToolExecution` weg**
   (4.2.1, 4.2.2, 4.3).
7. **Controller in den Runtime, eine Bauart** (3.1.4); `SecurityContext` aus
   den Entitäten (3.1.5); `SessionContext`/Instanziierung in den
   `SessionManager` (3.1.7).
8. **Store-Dialekt vereinheitlichen, User prüfen** (3.2.3, 3.2.4) — vor 03/04.
9. **`Ledger<X>` für Notes und Memory** (3.2.1) — als erster Schritt von 65,
   dort festhalten.
10. **REST:** `channel_for`/`readable_channels` in der Runtime,
    `open_channel`/`acting_user`, Response-Konstruktoren nach `models.rs`,
    eine `describe`-Funktion über `Channel`, `sse::stream`, ein Fehler-Mapper
    (5.2, 5.1.2–3); `subscribe_as` (5.4.1) mit Eintrag in 73.
11. **`ContextProvider`-Facette** (4.1.1) — Architekturentscheidung, nach
    Abstimmung; danach `Tags`/`Stages` und die asynchrone Factory (4.1.2–3).
12. **Die `pub`-Tabellen in einem Durchgang**, dann Querschnitt (serde,
    `unwrap`, Doku).
13. **Seed-Daten aus Server und TUI** (5.1.1) — getrennt planen.

Was hier nicht steht, weil ein Issue es hält, steht in Abschnitt 2.
