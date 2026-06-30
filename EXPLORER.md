# AstroParrot — L'Esploratore Autonomo (`SmartExplorer`)

Questo documento descrive **cosa fa** e **come lo fa** l'esploratore autonomo di
AstroParrot, implementato in [`src/explorer/smart_explorer.rs`](src/explorer/smart_explorer.rs).

L'esploratore è un attore che vive su un proprio thread, gira per la galassia,
raccoglie risorse, le combina in risorse complesse e sopravvive (in senso di
robustezza) a pianeti distrutti, energia mancante e canali che cadono — il tutto
in modo completamente automatico e senza mai andare in panic.

---

## 1. In una frase

> Ad ogni turno l'esploratore **scopre** cosa sa fare il pianeta su cui si trova,
> **spende l'energia disponibile** per craftare la risorsa più preziosa che riesce
> a raggiungere risalendo l'albero delle ricette, e quando non può più fare
> progressi (o è rimasto abbastanza a lungo) **viaggia** verso un pianeta nuovo.

---

## 2. Dove si colloca nell'architettura

Il gioco è un sistema ad attori che comunicano con canali `crossbeam`:

```
            ┌──────────────┐  BagContentRequest (= "fai il tuo turno")
            │ Orchestrator │ ───────────────────────────────────────┐
            │   (core.rs)  │ <───────── BagContentResponse / ...     │
            └──────┬───────┘                                         ▼
                   │ Neighbors / TravelToPlanet            ┌──────────────────┐
                   │ (handshake di viaggio)                │  SmartExplorer    │
                   ▼                                        │  (un thread)      │
            ┌──────────────┐  GenerateResource / Combine    │                   │
            │   Planet AI  │ <───────────────────────────── │  basics + complex │
            │  (tipo C)    │ ─────────────────────────────> │  (inventario)     │
            └──────────────┘  risorse generate / combinate   └──────────────────┘
```

L'esploratore implementa il trait `Explorer` definito in
[`src/explorer/mod.rs`](src/explorer/mod.rs):

```rust
pub trait Explorer {
    fn new(
        id: ID,
        current_planet: ID,
        rx_orchestrator: Receiver<OrchestratorToExplorer>,
        tx_orchestrator: Sender<ExplorerToOrchestrator<BagContent>>,
        tx_current_planet: Sender<ExplorerToPlanet>,
        rx_planet: Receiver<PlanetToExplorer>,
    ) -> Self;
    fn run(&mut self) -> Result<(), String>; // loop bloccante fino al kill
}
```

È un **drop-in replacement**: orchestratore, GUI e test continuano a funzionare
identici, semplicemente costruendo `SmartExplorer::new(...)` invece del vecchio
mock.

---

## 3. Il modello di gioco (riepilogo)

- **Pianeti** (tipo C in questa galassia): hanno **celle d'energia** caricate dai
  *sunray*. Ogni generazione/combinazione di risorse **consuma una cella carica**.
  Il pianeta crafta *per conto* dell'esploratore: l'esploratore manda gli
  ingredienti, il pianeta consuma una cella e restituisce il prodotto.
- **Risorse base**: `Carbon`, `Oxygen`, `Hydrogen`, `Silicon`.
- **Risorse complesse** e relative **ricette** (dalla `common-game`):

  | Risultato   | Ingredienti          |
  |-------------|----------------------|
  | `Diamond`   | `Carbon` + `Carbon`  |
  | `Water`     | `Hydrogen` + `Oxygen`|
  | `Life`      | `Water` + `Carbon`   |
  | `Robot`     | `Silicon` + `Life`   |
  | `Dolphin`   | `Water` + `Life`     |
  | `AIPartner` | `Robot` + `Diamond`  |

- Il pianeta AstroParrot (tipo C) genera `Carbon` e sa combinare `Diamond` e
  `AIPartner`. Ogni pianeta può però avere capacità diverse: per questo
  l'esploratore **non le assume mai**, le **chiede**.

---

## 4. Cosa fa, passo per passo (il turno)

Il "turno" dell'esploratore è scatenato dal messaggio
`OrchestratorToExplorer::BagContentRequest`. Il metodo `take_turn` esegue 4 fasi.

### Fase 1 — Scoperta del pianeta (`ensure_caps`)
Alla prima visita di un pianeta l'esploratore interroga:
- `SupportedResourceRequest` → quali **basi** può generare (`gens`),
- `SupportedCombinationRequest` → quali **complesse** può combinare (`combos`).

Il risultato è messo in cache e **invalidato solo quando viaggia**. Questo rende
l'esploratore **agnostico al pianeta**: funziona su qualunque configurazione di
ricette, non solo su Carbon→Diamond.

### Fase 2 — Produzione (`plan` + `generate`/`combine`)
L'esploratore chiede quante celle cariche ci sono e, finché ce ne sono (al massimo
`MAX_OPS_PER_TURN` passi), esegue **un passo di produzione per cella**:

1. **Scelta dell'obiettivo** — fra le complesse che *questo* pianeta sa combinare,
   ordina per "valore" (`value`): più la ricetta è profonda, più è preziosa
   (`AIPartner` > `Dolphin` > `Robot` > `Life` > `Water` > `Diamond`).
2. **Passo verso l'obiettivo** (`step_toward`, ricorsivo sull'albero delle
   ricette):
   - se ha già **entrambi gli ingredienti** in borsa → **combina** subito;
   - altrimenti produce il **primo ingrediente mancante**:
     - se è una **base** generabile qui → la **genera**;
     - se è una **complessa** combinabile qui → **scende ricorsivamente** verso di
       essa (es. per `Life` prima fa `Water`, per `Water` prima i due gas…).
   - se un ingrediente non è ottenibile su questo pianeta, scarta quell'obiettivo
     e prova il successivo.
3. **Fallback** — se nessuna complessa è raggiungibile qui, fa **scorta** (bounded
   a `MAX_BASIC_STOCK`) di una risorsa base generabile: potrà servire su un
   pianeta successivo, dato che l'inventario viaggia con lui.

Esempio concreto su un pianeta tipo C (genera `Carbon`, combina `Diamond`):
`step_toward(Diamond)` → se ha ≥2 `Carbon` combina un `Diamond`, altrimenti genera
un `Carbon`. Con una sola cella per turno la sequenza naturale è
`gen, gen, combine, gen, gen, combine, …` su più turni.

### Fase 3 — Viaggio (`travel`)
L'esploratore viaggia quando:
- **non ha potuto produrre nulla** in questo turno (pianeta esaurito o senza
  energia), **oppure**
- è rimasto sul pianeta da `STAY_LIMIT` turni (per continuare a *girare* per la
  galassia).

L'handshake con l'orchestratore è:
`NeighborsRequest` → `NeighborsResponse` → sceglie la destinazione → 
`TravelToPlanetRequest` → `MoveToPlanet` → aggiorna il canale verso il nuovo
pianeta → `MovedToPlanetResult`.

La **scelta della destinazione** preferisce i pianeti **non ancora visitati**
(insieme `visited`); se sono tutti già visti, ruota fra i vicini
(`travel_seq`). Così l'esploratore esplora davvero, invece di rimbalzare fra due
pianeti.

### Fase 4 — Resoconto (`report_bag`)
Chiude il turno inviando `BagContentResponse` con il contenuto della borsa
(`BagContent`: una mappa `ResourceType → quantità`), che la GUI mostra e
l'orchestratore conserva.

---

## 5. Come crafta davvero (dettaglio tecnico)

Per combinare, il pianeta ha bisogno degli **oggetti risorsa tipizzati**
(`Carbon`, `Water`, …), non di semplici conteggi. Per questo l'esploratore
conserva gli oggetti reali ricevuti dal pianeta:

```rust
basics: Vec<BasicResource>,
complexes: Vec<ComplexResource>,
```

`build_request` estrae dagli inventari gli ingredienti del tipo giusto e costruisce
la `ComplexResourceRequest` corretta per ognuna delle 6 ricette. Punto chiave di
sicurezza: **controlla la disponibilità prima di rimuovere** qualsiasi cosa, così
un fallimento parziale non perde mai una risorsa.

La tabella delle ricette è espressa una volta sola, in modo dichiarativo, nella
funzione `recipe`, che rispecchia le regole della `common-game`.

---

## 6. Come "sopravvive" — robustezza e resilienza

Nel gioco l'esploratore non ha vita/fame: *sopravvivere* significa **non morire mai
per errori tecnici** e **non restare bloccato**. Le garanzie:

- **Nessun panic.** Tutte le `send`/`recv` sono gestite; gli errori e i timeout
  diventano semplicemente "questo turno non faccio nulla".
- **Timeout sui canali.** `PLANET_TIMEOUT` (200 ms) e `ORCH_TIMEOUT` (500 ms)
  evitano blocchi indefiniti se un pianeta o l'orchestratore non rispondono.
- **Recupero ingredienti.** Se una combinazione fallisce, il pianeta restituisce i
  due ingredienti (`Err((msg, g1, g2))`): l'esploratore li **rimette in borsa**
  (`restore`).
- **No perdita di risorse senza energia.** Prima di combinare verifica che ci sia
  almeno una cella carica: un pianeta scarico scarterebbe silenziosamente la
  richiesta consumando gli ingredienti senza restituirli.
- **Rilocazione.** Se il suo pianeta viene distrutto da un asteroide,
  l'orchestratore lo sposta con `MoveToPlanet`: l'esploratore aggiorna il canale,
  invalida la cache delle capacità e prosegue sul nuovo pianeta.
- **Anti-stallo.** Se un pianeta non risponde alle query di capacità, la cache
  resta "sconosciuta", la produzione non parte e l'esploratore **viaggia** invece
  di restare fermo.
- **Galassia che si rimpicciolisce.** Continuando a muoversi verso pianeti vivi,
  l'esploratore resta sempre operativo finché esiste almeno un pianeta.

---

## 7. Modalità manuale (compatibilità completa col protocollo)

Oltre al turno autonomo, l'esploratore risponde a **tutti** i comandi diretti del
protocollo `OrchestratorToExplorer`, così funziona anche nella modalità "manuale"
dell'orchestratore e nei test end-to-end:

| Comando | Comportamento |
|---|---|
| `StartExplorerAI` / `StopExplorerAI` | ACK |
| `ResetExplorerAI` | svuota inventario e cache, poi ACK |
| `CurrentPlanetRequest` | risponde il pianeta corrente |
| `MoveToPlanet` | rilocazione: aggiorna canale e stato, poi ACK |
| `SupportedResourceRequest` / `SupportedCombinationRequest` | interroga il pianeta e risponde |
| `GenerateResourceRequest { to_generate }` | genera quella base, risponde Ok/Err |
| `CombineResourceRequest { to_generate }` | combina quella complessa, risponde Ok/Err |
| `BagContentRequest` | esegue un turno autonomo |

---

## 8. Parametri di comportamento (tutti in un punto)

In cima a `smart_explorer.rs`:

| Costante | Valore | Significato |
|---|---|---|
| `PLANET_TIMEOUT` | 200 ms | attesa massima di una risposta dal pianeta |
| `ORCH_TIMEOUT` | 500 ms | attesa massima durante l'handshake di viaggio |
| `STAY_LIMIT` | 3 | turni dopo i quali viaggia comunque (per esplorare) |
| `MAX_OPS_PER_TURN` | 8 | tetto ai passi di produzione per turno |
| `MAX_BASIC_STOCK` | 3 | quante basi "senza sbocco" accumulare al massimo |

Cambiare questi valori regola quanto l'esploratore è aggressivo nel craftare vs.
nell'esplorare, senza toccare la logica.

---

## 9. Perché il codice resta semplice

- **Una sola struct, un solo file**, con metodi piccoli e a responsabilità unica.
- La strategia di crafting è **data-driven**: la tabella `recipe` + la funzione
  `value` descrivono *cosa* fare; `step_toward` è una ricorsione di poche righe che
  decide *come*. Aggiungere/cambiare ricette non richiede toccare la logica.
- **Un passo di produzione per cella d'energia**: mappa naturalmente sul modello a
  turni del gioco, niente pianificatori complessi.
- Nessun `unwrap`/`panic` nei percorsi di esecuzione: solo `Option`/`Result`
  gestiti.

---

## 10. Come eseguire e verificare

```bash
# Test end-to-end (orchestratore ↔ esploratore ↔ pianeta), senza GUI:
cargo test --test integration --no-default-features

# Build/lint completi, GUI inclusa:
cargo build
cargo clippy

# Gioco con interfaccia grafica (esploratori autonomi che girano per la galassia):
cargo run --features game
```

**Unit test** della logica decisionale (in `smart_explorer.rs`, eseguiti in
isolamento, senza thread): le ricette coincidono con quelle della `common-game`,
l'ordinamento per valore, la scelta del passo verso un `Diamond`, la **risalita
ricorsiva** dell'albero (`Life → Water → gas`), la preferenza per l'obiettivo più
prezioso, il progresso parziale quando l'obiettivo top è solo in parte
raggiungibile, lo *stockpiling* di fallback e il caso "pianeta inutilizzabile".

**Test d'integrazione** (orchestratore ↔ esploratore ↔ pianeta reale):
- `orchestrator_explorer_planet_pipeline`: avvio pianeta, registrazione,
  generazione `Carbon`, crafting `Diamond` (via comando), difesa dall'asteroide.
- `explorer_autonomously_travels`: l'esploratore chiede i vicini e viaggia
  autonomamente, completando l'handshake.
- `explorer_autonomously_crafts_a_diamond`: prova il **cervello autonomo** — con i
  soli turni e i sunray, l'esploratore scopre le ricette, mina `Carbon` e crafta un
  `Diamond` da solo, senza comandi manuali.

Stato attuale: **build pulita, nessun warning di clippy (default, anche sui test),
11/11 test verdi** (8 unit + 3 integrazione).
