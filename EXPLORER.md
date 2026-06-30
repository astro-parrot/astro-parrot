# AstroParrot — L'Esploratore Autonomo (`SmartExplorer`)

Questo documento descrive **cosa fa** e **come lo fa** l'esploratore autonomo di
AstroParrot, implementato in [`src/explorer/smart_explorer.rs`](src/explorer/smart_explorer.rs).

L'esploratore è un attore che vive su un proprio thread e gira per la galassia con
**una sola ossessione: collezionare Diamanti**. Ignora ogni altra risorsa, mina il
Carbon che gli serve, lo fonde in Diamanti e — raggiunta la quota — smette di
raccogliere e si limita a vagare ammirando il bottino. Tutto in modo automatico,
robusto (sopravvive a pianeti distrutti, energia mancante, canali che cadono) e
senza mai andare in panic.

---

## 1. In una frase

> Ad ogni turno l'esploratore **scopre** cosa sa fare il pianeta su cui si trova e
> fa **una sola mossa** verso il prossimo Diamante (mina un Carbon, oppure fonde due
> Carbon in un Diamante); se il pianeta non può aiutarlo **viaggia** altrove, e una
> volta collezionati `TARGET_DIAMONDS` Diamanti entra in *museum mode* e si limita a
> girare per la galassia.

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
identici, semplicemente costruendo `SmartExplorer::new(...)`.

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

- La ricetta che interessa al collezionista è **una sola**: `Diamond = Carbon +
  Carbon`. Tutto il resto della tabella esiste solo per la modalità manuale
  (vedi §7). L'esploratore **non assume mai** le capacità di un pianeta: le **chiede**.

---

## 4. Cosa fa, passo per passo (il turno)

Il "turno" è scatenato dal messaggio `OrchestratorToExplorer::BagContentRequest`.
Il metodo `take_turn` fa esattamente tre cose: **scopre**, **decide una mossa**,
**la esegue** e infine **riporta la borsa**.

### Passo 1 — Scoperta del pianeta (`ensure_caps`)
Alla prima visita di un pianeta l'esploratore interroga:

- `SupportedResourceRequest` → quali **basi** può generare (`gens`),
- `SupportedCombinationRequest` → quali **complesse** può combinare (`combos`).

Il risultato è messo in cache e **invalidato solo quando viaggia**. Così
l'esploratore è **agnostico al pianeta**: scopre da solo se *questo* pianeta sa
minare Carbon e/o fondere Diamanti.

### Passo 2 — Decisione della mossa (`decide`)
Questa è la "testa" dell'esploratore. È una funzione **pura** (non tocca i canali,
legge solo la borsa e le capacità del pianeta) e restituisce **una** delle tre
mosse possibili, valutando le condizioni **in quest'ordine**:

1. **Ossessione soddisfatta?** Se in borsa ci sono già `TARGET_DIAMONDS` Diamanti
   → `Move::Wander` (museum mode: la collezione è completa, smette di produrre).
2. **Posso forgiare qui e ora?** Se il pianeta sa combinare `Diamond` **e** ho
   almeno **2 Carbon** in borsa → `Move::Forge`.
3. **Mi serve altro Carbon e qui lo posso minare?** Se il pianeta genera `Carbon`
   **e** ne ho meno di 2 → `Move::Mine`. *Ogni altra risorsa offerta dal pianeta
   viene deliberatamente ignorata.*
4. **Altrimenti** (pianeta inutile per la mia ossessione) → `Move::Wander`.

### Passo 3 — Esecuzione della mossa (`take_turn`)
A seconda di cosa ha deciso:

- **`Forge`** → chiede al pianeta di combinare un `Diamond` (`combine`). Se
  fallisce (es. niente energia), **viaggia** invece di restare bloccato.
- **`Mine`** → chiede al pianeta di generare un `Carbon` (`generate`). Se fallisce,
  **viaggia**.
- **`Wander`** → **viaggia** subito (sia per cercare un pianeta utile, sia per
  vagare in museum mode).

Una mossa per turno: con una cella d'energia per turno questo mappa naturalmente
sul ritmo del gioco. La sequenza tipica su un pianeta che mina Carbon e fonde
Diamanti è quindi `mina, mina, forgia, mina, mina, forgia, …` su più turni, fino a
5 Diamanti.

### Passo 4 — Viaggio (`travel`)
Quando deve spostarsi, l'handshake con l'orchestratore è:
`NeighborsRequest` → `NeighborsResponse` → sceglie la destinazione →
`TravelToPlanetRequest` → `MoveToPlanet` → aggiorna il canale verso il nuovo
pianeta → `MovedToPlanetResult`.

La **scelta della destinazione** preferisce i pianeti **non ancora visitati**
(insieme `visited`); se sono tutti già visti, ruota fra i vicini (`travel_seq`).
Così l'esploratore esplora davvero, invece di rimbalzare fra due pianeti. All'arrivo
(`arrive`) **invalida la cache** delle capacità, perché il nuovo pianeta potrebbe
saper fare cose diverse.

### Passo 5 — Resoconto (`report_bag`)
Chiude il turno inviando `BagContentResponse` con il contenuto della borsa
(`BagContent`: una mappa `ResourceType → quantità`), che la GUI mostra e
l'orchestratore conserva.

---

## 5. Come forgia davvero (dettaglio tecnico)

Per combinare, il pianeta ha bisogno degli **oggetti risorsa tipizzati** (i due
`Carbon`), non di semplici conteggi. Per questo l'esploratore conserva gli oggetti
reali ricevuti dal pianeta:

```rust
basics: Vec<BasicResource>,
complexes: Vec<ComplexResource>,
```

`build_request` estrae dall'inventario gli ingredienti del tipo giusto e costruisce
la `ComplexResourceRequest`. Punto chiave di sicurezza: **controlla la disponibilità
prima di rimuovere** qualsiasi cosa, così un fallimento parziale non perde mai una
risorsa. La funzione resta scritta per **tutte e 6** le ricette così la "combine"
manuale (§7) continua a funzionare per qualunque risorsa, ma il cervello autonomo
chiama solo `combine(Diamond)`.

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
  resta "sconosciuta", la mossa fallisce e l'esploratore **viaggia** invece di
  restare fermo.
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
| `TARGET_DIAMONDS` | 5 | quanti Diamanti collezionare prima del *museum mode* |

Cambiare `TARGET_DIAMONDS` regola quanto a lungo l'esploratore resta "ossessionato"
prima di mettersi a riposo, senza toccare la logica.

---

## 9. Perché il codice resta semplice

- **Una sola struct, un solo file**, con metodi piccoli e a responsabilità unica.
- La strategia è **una sola ricetta** (`Diamond = Carbon + Carbon`): niente
  pianificatori, niente ricorsione sull'albero delle ricette.
- **Decisione separata dall'azione**: `decide` è pura e restituisce un `Move`
  (`Forge` / `Mine` / `Wander`); `take_turn` la esegue. Questo rende la logica
  banale da leggere e da testare in isolamento.
- **Una mossa per cella d'energia**: mappa naturalmente sul modello a turni del
  gioco.
- Nessun `unwrap`/`panic` nei percorsi di esecuzione: solo `Option`/`Result`
  gestiti.

---

## 10. Come eseguire e verificare

```bash
# Tutti i test (unit + integrazione):
cargo test

# Build/lint completi, GUI inclusa:
cargo build
cargo clippy

# Gioco con interfaccia grafica (esploratori autonomi che girano per la galassia):
cargo run --features game
```

**Unit test** della logica decisionale (in `smart_explorer.rs`, eseguiti in
isolamento, senza thread, con borsa vuota — bastano a verificare *quale* mossa
sceglie `decide` date le capacità del pianeta):

- mina `Carbon` su un pianeta che sa fondere Diamanti;
- mina `Carbon` anche dove non può fonderlo (se lo porta dietro);
- ignora ogni risorsa che non sia `Carbon` (pianeta con solo acqua/gas → vaga);
- vaga via da un pianeta inutilizzabile;
- la ricetta del `Diamond` è due `Carbon`.

**Test d'integrazione** (orchestratore ↔ esploratore ↔ pianeta reale):

- `orchestrator_explorer_planet_pipeline`: avvio pianeta, registrazione,
  generazione `Carbon`, crafting `Diamond` (via comando), difesa dall'asteroide.
- `explorer_autonomously_travels`: l'esploratore chiede i vicini e viaggia
  autonomamente, completando l'handshake.
- `explorer_autonomously_crafts_a_diamond`: prova il **cervello autonomo** — con i
  soli turni e i sunray, l'esploratore scopre le ricette, mina `Carbon` e forgia un
  `Diamond` da solo, senza comandi manuali.

Stato attuale: **build pulita, 8/8 test verdi** (5 unit + 3 integrazione).
