# Stormfront — tempestades e tornados para Horizon (Minecraft em Rust)

Mod nativo para o [Horizon](https://github.com/horizon-rust/horizon) (Minecraft: Java Edition 26.3
reescrito em Rust): atmosfera simplificada, supercélulas e outros modos de tempestade, tornados com
campo de vento 3D, destruição baseada em pressão dinâmica e resistência dos materiais, detritos
físicos com fragmentação, danos à vegetação (incluindo descascamento), *ground scouring*, radar
Doppler (REF, VEL, CC) e levantamento pós-evento no estilo Enhanced Fujita.

> **Estado: Fase 1 a 5 implementadas no núcleo de simulação, integração com o Horizon escrita e
> verificada por compilação contra as assinaturas reais, mas ainda não compilada dentro do Horizon
> nem jogada.** Veja [Verificação](#verificação) para o que foi e o que não foi testado.

## Duas versões

| Versão | Arquivo | Para | Como funciona |
| --- | --- | --- | --- |
| **Stormfront** (completa) | `stormfront-0.1.0.hnmod` (`python3 tools/package.py`) | Horizon de PC com o launcher oficial | mod nativo compilado dentro do jogo: danos lendo cada bloco, detritos físicos, partículas 3D próprias, levantamento EF |
| **Stormfront Lite** | `stormfront_lite-0.1.0.hmod` (`android/build.sh`) | **Horizon Android** (port NepThunder 0.7.x) e outros que rodam mods `.hmod` em WebAssembly | mod WASM de cliente: mesma atmosfera, tempestades, vórtice 3D e radar; danos por `fill ... replace <material>` onde o vento simulado vence a resistência daquele material; partículas, detritos, chuva e raios vanilla |

O Horizon Android não compila mods nativos (`.hnmod` é recusado: "wrong extension"); ele roda
`.hmod` WebAssembly em sandbox. Por isso existe a versão Lite.

### Instalar no Horizon Android

1. Gere com `android/build.sh` (ou baixe o `.hmod`).
2. No app: **Installed → Import Mod** e escolha `stormfront_lite-0.1.0.hmod`.
3. Em **Installed → Mods**, ligue o Stormfront Lite (pacotes novos vêm desligados) e reinicie o mundo.
4. Use um mundo **com cheats** (o mod executa comandos vanilla como você) e descartável:
   `/storm env outbreak`, `/tornado spawn wedge`, `/radar open`, `/radar product velocity`.

Comandos da Lite: `/storm <tipo>|env|debug|clear|tornado`, `/tornado spawn <forma>|intensity|speed|multivortex|damage|dissipate|list|survey`,
`/radar open|close|product|range|tilt|here`, `/sfx particles low|medium|high|lightning|sound|shake|weather on|off`.

Verificado aqui: o componente foi rodado num host `wasmtime 48.0.8` (a versão do port) com os limites
do jogo (fuel por tick, memória, 256 comandos, 8 linhas de chat, 1024 formas): 60 s de tornado wedge
com radar ligado sem nenhum *trap*, pico de 39% do fuel do tick. Não foi testado no celular.

O que a Lite não tem: leitura de blocos (logo, sem colapso estrutural nem classificação EF por danos
— o levantamento mostra trajeto, largura e vento), partículas que seguem o vento continuamente (são
partículas vanilla lançadas com a velocidade do vento), escavação profunda do solo.

## O que foi encontrado no Horizon (antes de programar)

Inspecionei o código-fonte real (`horizon-rust/horizon`, Nightly `nightly-source/2026-11-05-f319701`):

- **Não há Forge/Fabric/NeoForge nem Java.** Mods de código são *native mods*: pacotes `.hnmod` que o
  launcher compila **dentro do executável do jogo** com o
  [horizon-mixins](https://github.com/horizon-rust/horizon-mixins), que injeta *helpers* Rust em
  funções do jogo (HEAD/RETURN/…) e adiciona módulos e crates ao workspace.
- **API estável = catálogo de hook points** (`native-hooks.json`, 427 hooks, `NATIVE_HOOKS_API 409`
  na Nightly fixada) mais *helpers* `Game::native_*` mantidos estáveis. O Stormfront usa **apenas**
  hooks do catálogo e helpers — nenhuma função interna "crua".
- **Mundo e chunks:** o servidor autoritativo (thread do servidor no single player, ou servidor
  dedicado) expõe leitura em massa de colunas (`native_read_columns`, até 1 048 576 blocos por
  chamada) e edição em massa direto nas seções (`native_edit_blocks`, sem atualizar vizinhos, com
  reenvio dos chunks alterados). Escrever bloco a bloco seria lento; o mod usa só as versões em massa.
- **Rede:** mensagens de mod servidor→cliente (`native_net_send_to_player`, 30 000 bytes, 32 por
  jogador por tick). No single player o servidor integrado também é um *host* LAN, então funciona igual.
- **Renderização:** wgpu 30 (Vulkan/Metal/DX12/GLES). O hook `world_drawn` entrega device, queue,
  encoder, alvo de cor, profundidade invertida e matrizes — dá para desenhar um passe próprio.
- **Partículas do motor** existem (`ParticleEngine::controlled`), mas são sprites limitados pelo
  orçamento do vanilla; o tornado usa um passe instanciado próprio.
- **Comandos:** servidor (`register_commands`/`perform_command`) e cliente (`commands`/`command`).
  **`/weather` é do vanilla e não pode ser assumido por mod**, então os comandos de tempestade ficaram
  em `/storm`.
- **Entidades de detrito:** existe `native_spawn_falling_block`, mas ele não fragmenta nem segue um
  campo de vento; os detritos são simulados pelo próprio mod e enviados aos clientes.

## Arquitetura

```
bundle/stormfront/                 o pacote nativo (vira mixins/stormfront/ no .hnmod)
  horizon-mixin.toml               14 injeções em hooks do catálogo, 2 módulos, 1 crate
  engine/                          crate `stormfront`: só biblioteca padrão, testável sem Horizon
    src/atmosphere/                sondagem, termodinâmica, CAPE/CIN/LCL, shear, SRH, Bunkers, STP, SCP, DCAPE
    src/storms/                    célula, multicélula, supercélula clássica/HP/LP, QLCS, microbursts,
                                   mesociclone, RFD/FFD, hidrometeoros, ventos de escala de tempestade,
                                   tornadogênese probabilística
    src/tornado/                   vortex.rs (campo 3D), multivortex.rs, condensation.rs, dynamics.rs
    src/damage/                    materials.rs, world.rs (cache de colunas), structures.rs,
                                   vegetation.rs, ground_scouring.rs, debris.rs, survey.rs, engine.rs
    src/radar/                     reflectivity.rs, velocity.rs, correlation.rs, scanning.rs, display.rs
    src/particles/                 sistema de partículas + emissores (funil, nuvem de detritos, base,
                                   wall cloud, clear slot, chuva, granizo, cortinas, raios)
    src/sim.rs, src/client.rs      metades servidor e cliente (independentes do motor)
    src/net.rs, commands.rs, audio.rs, hud.rs, render_bridge.rs, config.rs
    data/particles.wgsl            shader das partículas
    examples/bench.rs              benchmark de CPU
  glue.rs                          liga o crate aos hooks do jogo (módulo em game.rs)
  render.rs                        passe wgpu no hook world_drawn (módulo em horizon-client-render)
  *.rs                             helpers de uma linha, um por hook
android/                           Stormfront Lite (WASM .hmod para Horizon Android)
  stormfront-lite/                 o mod: lite.rs (lógica) e lib.rs (bindings WIT do Horizon)
  tools/                           componentiza com wit-component 0.254.2, como o Horizon faz
  hosttest/                        roda o componente em wasmtime 48.0.8 com os limites do jogo
  build.sh                         testes, build, checagem e target/stormfront_lite-<versão>.hmod
tools/package.py                   gera target/stormfront-<versão>.hnmod reprodutível
tools/verify-horizon.sh            prepara (stage) e compila o Horizon real com o mod
docs/ARQUITETURA.md                fluxo de dados, threads, protocolo e modelos físicos
```

Física e visual estão **desacoplados**: o servidor simula clima, vento e danos; o cliente
reconstrói tempestades e tornados a partir das mensagens e avalia **o mesmo campo de vento** para as
partículas e o radar. A qualidade visual (`/sfx quality`) muda só a quantidade de partículas e a
resolução do radar, nunca os danos.

## Comandos

Servidor (operador nível 2 para alterar o mundo):

| Comando | Efeito |
| --- | --- |
| `/storm supercell` · `/storm hp` · `/storm lp` · `/storm multicell` · `/storm single` · `/storm qlcs` `[distância]` | forma a tempestade a montante, vindo na direção do jogador (`/weather storm …` da proposta) |
| `/storm microburst` | célula em colapso com *downburst* |
| `/storm env classic\|outbreak\|hp\|lp\|qlcs\|pulse\|microburst\|marginal` | troca a sondagem (os parâmetros decidem o modo das tempestades) |
| `/storm debug` | CAPE, CIN, LCL, LFC, EL, shear 0–1/0–3/0–6 km, SRH, STP, SCP, estado das tempestades e tornados (`/weather debug`) |
| `/storm tornado` · `/storm clear` · `/storm natural on\|off` · `/storm timescale <x>` | força tornadogênese, limpa, formação espontânea, escala de tempo |
| `/tornado spawn rope\|cone\|stovepipe\|wedge\|elephant\|rainwrapped\|multivortex\|satellite` | tornado com condições iniciais que levam à morfologia |
| `/tornado intensity 85` · `/tornado speed 12` · `/tornado multivortex true` · `/tornado damage on` | ajustes (m/s internamente; a resposta mostra km/h) |
| `/tornado survey [id]` · `/tornado list` · `/tornado track [id]` · `/tornado dissipate` | levantamento, lista, trajetória, dissipação |

Cliente:

| Comando | Efeito |
| --- | --- |
| `/radar open\|close` · `/radar product reflectivity\|velocity\|cc` | painel PPI e produto |
| `/radar tilt <n>` · `/radar range <km>` · `/radar history <n>` · `/radar expand` | elevação, alcance, histórico, painel ampliado |
| `/radar here` · `/radar band S\|C\|X` · `/radar aliasing on\|off` | instala o radar onde você está, banda (atenuação), aliasing |
| `/sfx quality low\|medium\|high` · `/sfx science on\|off` · `/sfx track on\|off` · `/sfx shake\|sound on\|off` | qualidade, modo científico, trajetórias coloridas por intensidade, tremor e som |

Os relatórios de levantamento são salvos em `config/stormfront/tornado-<id>.txt` e a trajetória em
`tornado-<id>-track.csv`.

## Instalação

1. Gere o pacote: `python3 tools/package.py` → `target/stormfront-0.1.0.hnmod`.
2. No launcher do Horizon (0.2.37+), abra a pasta da instância e coloque o arquivo em
   `horizon-native-mods/`.
3. Na página **Mods**, confirme que o Stormfront está ligado; em **Native build** clique em **Build** e
   aceite o aviso de código nativo (mods nativos não rodam em sandbox — leia o código antes).
4. Jogue no Overworld: `/storm env outbreak`, `/tornado spawn wedge`, `/radar open`.

O `.hnmod` declara `"hooks": 409`: compila na Nightly 2026-11-05 ou mais nova.

## Verificação

O que foi executado neste repositório:

- **114 testes** do crate de simulação (`cargo test --release`), incluindo: continuidade de massa do
  vórtice, déficit de pressão contra integração numérica, ordem dos graus de dano (vidro < cobertura <
  paredes < piso), casa destruída progressivamente por um tornado violento e só danificada levemente
  por um fraco, nem toda supercélula produzir tornado, TDS só aparecer com detritos lançados, velocidade
  radial ser só a componente do feixe, aliasing, ida e volta do protocolo, servidor→cliente de ponta a ponta.
- **Shader WGSL validado com `naga 30.0.1`** (a mesma versão que o Horizon fixa).
- **`glue.rs`, `render.rs` e os 14 helpers compilam sem avisos** contra um *mock* do Horizon cujas
  assinaturas foram copiadas da Nightly fixada e contra o crate real **`wgpu 30.0.1`**; os 14 hooks
  existem com versão 1 no catálogo da Nightly.
- **Benchmark de CPU** (`cargo run --release --example bench`, x86 deste ambiente, não Android): tick do
  servidor com tornado wedge de 95 m/s cruzando uma vila — mediana 2,6 ms, p95 3,3 ms, pico 12 ms,
  cache limitado a ~29 MB; tick do cliente 0,2 ms (low, ~1,5 mil partículas), 0,6–0,8 ms (medium,
  ~4–5 mil), 1,5 ms (high, ~10 mil); varredura de radar 1,4 / 5,5 / 12 ms numa thread separada.

O que **não** foi feito aqui: compilar o Horizon com o mod aplicado (o ambiente bloqueou compilar e
executar os repositórios de terceiros) e jogar. Para isso:

```sh
git clone https://github.com/horizon-rust/horizon ../horizon
git clone https://github.com/horizon-rust/horizon-mixins ../horizon-mixins
tools/verify-horizon.sh ../horizon ../horizon-mixins check   # ou build
```

Desempenho no Android ainda precisa ser medido no aparelho; os números acima não garantem 60 FPS.

## Fases

- [x] **Fase 1** — inspeção do Horizon, integração por hooks, circulação, campo de vento 3D, funil visual, leitura/escrita de blocos em massa
- [x] **Fase 2** — materiais, destruição progressiva e em cadeia, detritos com fragmentação, vegetação (desfolhamento, quebra, arranque, descascamento), *scouring*
- [x] **Fase 3** — atmosfera, tempestades, mesociclones, ciclo de vida, tornadogênese e dissipação
- [x] **Fase 4** — REF, VEL, CC, TDS, detecção de mesociclone/TVS, interface, histórico, elevações, bandas
- [x] **Fase 5** — morfologias emergentes, subvórtices, condensação variável, cortinas de chuva, poeira, sons espaciais
- [ ] **Fase 6** — build dentro do Horizon, benchmarks no Android, testes de salvamento/carregamento e de chunks no jogo real, ajustes finos

## Limitações conhecidas

- O radar é instalado com `/radar here` (posição guardada no cliente); ainda não há bloco de radar
  (precisaria de um mod de conteúdo `.hmod` com o bloco e textura, mais o hook `right_click_block`).
- Sons usam eventos do vanilla (elytra para o rugido, trovão, vidro, madeira); não há sons próprios.
- Tempestades e tornados ativos não sobrevivem a salvar e recarregar o mundo; as mudanças no
  terreno e os relatórios, sim.
- Só o Overworld tem clima.
- Em tornados muito largos o cache de colunas (padrão 180 000) corta a franja externa de ventos mais
  fracos; `ServerSettings::max_cached_columns` aumenta o limite.
- A escala de tempestade é comprimida (`WorldScale`: comprimentos ×0,25, tempo ×4, base das nuvens
  ×0,15 na visualização) para caber no mundo; tornados e danos ficam em escala real (1 bloco = 1 m).

## Licença

MIT.
