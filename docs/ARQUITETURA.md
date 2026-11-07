# Stormfront — arquitetura e modelos físicos

## Threads e fluxo de dados

```
 thread do servidor (single player: thread integrada; ou servidor dedicado)
   level_tick_post("minecraft:overworld")
     ServerSim::tick  (passo fixo de 0,05 s)
       StormSystem::step       atmosfera → tempestades → tornadogênese → tornados
       DamageEngine::tick      lê chunks (native_read_columns) → avalia colunas → falhas,
                               árvores, scouring, colapsos → detritos → grava em lote
                               (native_edit_blocks a cada 4 ticks)
       registros dos tornados  trajetória, danos por célula de 8 m, relatórios
     mensagens → native_net_send_to_player (canal stormfront:sync)

 thread da janela (cliente)
   net_from_server → ClientState::on_message   réplica do clima, detritos, efeitos
   tick           → ClientState::tick          extrapola a réplica, partículas, radar,
                                               sons (native_play_local_sound), tremor
                    render_bridge::publish     instâncias para a GPU
   hud            → painel do radar, aviso, leitura científica (native_fill/native_text)

 thread "stormfront-radar"
   compute_sweep sobre uma cópia (snapshot) do clima → varreduras e detecções

 thread de render
   render_needs   → pede a profundidade do mundo
   world_drawn    → passe wgpu instanciado sobre o alvo de cor
```

Nenhum estado é compartilhado entre a metade servidor e a metade cliente além das mensagens, então o
mesmo código serve single player, LAN e servidor dedicado.

## Hooks usados (Nightly 2026-11-05, todos versão 1)

| Hook | Lado | Uso |
| --- | --- | --- |
| `level_tick_post` | servidor | simulação do Overworld e envio das mensagens |
| `register_commands` / `perform_command` | servidor | `/storm`, `/tornado` |
| `world_lifecycle` | servidor | zera a simulação ao abrir/fechar mundo |
| `net_from_server` | cliente | recebe clima, detritos, efeitos e trajetórias |
| `tick` | ambos (age só na janela) | partículas, radar, sons |
| `hud` | cliente | painel do radar, aviso, modo científico |
| `commands` / `command` | cliente | `/radar`, `/sfx` |
| `camera_shake` | cliente | tremor perto do tornado |
| `client_level_unload` | cliente | descarta o estado ao sair do nível |
| `mods` | cliente | linha em Opções → Mods |
| `world_drawn` / `render_needs` | render | passe de partículas, profundidade |

## Protocolo (`net.rs`)

Mensagens binárias little-endian com versão. Tipo 1: ambiente (preset + especificação da sondagem) e
todas as tempestades e tornados com os parâmetros necessários para reavaliar campos de vento e
hidrometeoros (posições em f64). Tipo 2: detritos próximos (tabela de estados de bloco + posição,
velocidade, tamanho). Tipo 3: efeitos (estouros de bloco, queda de árvore, colapso, fragmentação,
raios, avisos). Tipo 4: trajetória (x, z, Vmax). Todas cabem em 30 000 bytes; detritos e efeitos são
divididos quando necessário.

## Modelos físicos

### Atmosfera (`atmosphere/`)
Sondagem paramétrica a cada 100 m até 16 km (temperatura, ponto de orvalho, pressão hidrostática com
temperatura virtual, hodógrafo). Parcela levantada adiabaticamente seca até o LCL (Bolton 1980) e
pseudo-adiabaticamente acima → CAPE, CIN, LFC, EL, CAPE 0–3 km. Bunkers (direita/esquerda), SRH 0–1 e
0–3 km, shear 0–1/0–3/0–6 km, STP (forma de camada fixa com termo de CIN), SCP, DCAPE (θe mínimo nos
400 hPa inferiores), água precipitável, índice de microburst (seco: DCAPE × lapse rate subnuvem ×
depressão do ponto de orvalho; úmido: CAPE × PW × shear fraco).

### Tempestades (`storms/`)
O modo vem do ambiente (shear 0–6 km, SRH, CAPE, vento médio; HP/LP pela eficiência de precipitação e
pelo vento relativo na bigorna). Ciclo de vida: desenvolvimento → maturidade → intensificação →
tornádica → oclusão → dissipação. O mesociclone cresce com SRH e shear (níveis médios) e com SRH 0–1 km,
CAPE baixo e LCL baixo (níveis baixos); a RFD envolve o mesociclone (oclusão) e um novo mesociclone pode
se formar (ciclos). Hidrometeoros paramétricos: núcleo do flanco dianteiro carregado pela bigorna,
*hook echo* em arco atrás do mesociclone, núcleo de granizo, BWER sob a saliência de eco, bigorna,
linha convectiva com região estratiforme e banda brilhante (QLCS). Tornadogênese: chance por minuto ∝
STP e intensidade do mesociclone, reduzida por LCL alto e em LP/QLCS; a intensidade sorteada fica
quase sempre abaixo do potencial do ambiente, e >100 m/s exige ambiente extremo e sorteio raro.

### Vórtice (`tornado/vortex.rs`)
Velocidade tangencial Burgers–Rott, `V(r) = Vmax/0,638 · (1 − e^{−r²/rc²}) / (r/rc)` (pico em
r = 1,12 rc), ou Rankine modificado com decaimento `r^{−α}`; camada superficial em lei de potência
(expoente 0,11). Componentes radial e vertical de uma função de corrente
`ψ = A·F(r)·G(z)`, `F = rc²[(1 − e^{−y}) − β y e^{−y}]`, `G = 1 − e^{−z/δ}` — continuidade de massa
exata (testada), jato de entrada junto ao solo e corrente ascendente no núcleo (β = 0, uma célula) ou
descendente central com anel ascendente (β > 1, duas células). Translação somada; assimetria de número
de onda 1 no flanco traseiro; rajadas por processo de Ornstein–Uhlenbeck. Perto do núcleo, o campo do
tornado substitui (não soma) o fluxo ambiente.

### Subvórtices (`tornado/multivortex.rs`)
Em estrutura de duas células surgem 2–6 vórtices de sucção que orbitam perto de Rmax a ~55% da
velocidade de pico, com núcleos de 8–16% de Rmax, 30–50% de Vmax e vida de 8–35 s; somam-se ao campo
principal e produzem faixas estreitas de vento extremo (e marcas cicloidais no solo).

### Funil de condensação (`tornado/condensation.rs`)
Déficit de pressão ciclostrófico `Δp(r) = ρ ∫_r^∞ V²/r' dr'` (tabelado para Burgers–Rott, fechado para
Rankine). A altura f·H da tela representa f·LCL; condensa onde `Δp ≥ ρ g (LCL_entrada)(1 − f)`, com o
LCL do ar ingerido = 0,5 × LCL ambiente (ar úmido convergente e ar da RFD). O vórtice se alarga para
cima conservando circulação (cone). Largura do funil ≠ largura dos ventos danosos. Morfologia
classificada pelo próprio funil: corda, cone, chaminé, cunha, tromba de elefante, múltiplos vórtices,
envolto em chuva, funil suspenso.

### Danos (`damage/`)
- **Carga:** `q = ½ρV²` × coeficiente de pressão somado nas faces expostas (barlavento 0,8, sotavento
  0,5, laterais, sucção no topo 0,9, empuxo por baixo, 0,25 em faces voltadas para ar interno).
- **Resistência:** resistência do material × (1 + conectividade × apoio) — apoio por baixo, vizinhos
  na parede, carga acima; telhados presos só por fixações; ancoragem ×2,5 sobre o terreno; balanços
  ×0,4; qualidade por bloco (0,8–1,25) e por edificação (0,7–1,35); fadiga sob carga sustentada.
  Calibrado para a ordem dos graus de dano EF (vidro ~35 m/s, cobertura ~40 m/s, estrutura do telhado
  ~55 m/s, paredes de madeira ~60 m/s, alvenaria ~75 m/s, concreto muito acima).
- **Progressão e cadeia:** cada perda muda exposição e apoio dos vizinhos; componentes estruturais sem
  caminho até o terreno caem (busca limitada).
- **Vegetação:** folhas pelo modelo genérico (desfolhamento de fora para dentro); tronco pela vela da
  copa — vento crítico por espécie, maior com copa perdida; quebra em altura aleatória (12–60%) ou
  arranque em solo úmido com raízes rasas; tronco deitado na direção do vento abaixo de 75 m/s, em
  pedaços voando acima; descascamento por bloco acima de ~62 m/s com abrasão de detritos (blocos
  `stripped_*`).
- **Scouring:** dose de erosão `∫ e·(U − U_lim)² dt` por coluna, com erodibilidade do solo e umidade;
  limiares 48/62/74/85 m/s para grama→terra, solo superficial, escavação e sulcos; manchas irregulares
  por ruído espacial; profundidade máxima de 3 blocos.
- **Detritos:** massa pontual com arrasto `½ρ Cd A |u−v|(u−v)` e gravidade; colisão com o mundo;
  energia de impacto quebra o alvo (vidro primeiro) e fragmenta a peça (energia específica >
  limiar do material, até 3 gerações); peças pesadas em repouso viram depósito (tábuas viram lajes,
  tijolos, troncos deitados). Massa de "construção" por bloco (seção de parede de 1 m², não cubo maciço).
- **Orçamento:** leitura por chunks mais próximos, avaliação priorizada por atraso × intensidade,
  cache limitado (`max_cached_columns`), escrita em lote.

### Radar (`radar/`)
Feixe com raio terrestre efetivo 4/3, largura ~0,95° e subamostras ponderadas pela potência dentro
do volume de resolução. Z da chuva `3,63·10⁹ (ρq)^1,75`, neve −7 dB com banda brilhante, granizo e
detritos (∝ concentração de massa). Velocidade radial ponderada por Z com velocidades de queda
(chuva 6, neve 1, granizo 15 m/s; detritos atrasados), ruído dependente da SNR, aliasing opcional.
CC da mistura ponderada por Z com penalidade de diversidade (chuva 0,99, granizo 0,93, detritos 0,45).
Atenuação por banda (S/C/X), limiar de ruído crescente com a distância. Detecção de pares de
velocidade (MESO/TVS por diferença gate-a-gate) e TDS (CC baixo com refletividade, junto ao par).

### Levantamento (`damage/survey.rs`)
Células de 8 m guardam indicadores de dano (residência de madeira FR12, alvenaria/concreto, árvores de
madeira dura e coníferas), o maior grau de dano visto e a qualidade da construção. A classificação usa o
vento esperado do grau de dano (limite inferior/superior por qualidade) — nunca a velocidade simulada.
Sem indicadores, o resultado é EFU. O relatório inclui intensidade estimada, comprimento, largura, mapa
ASCII de danos, scouring, descascamento, detritos e extremos de vento.

## Escalas

`WorldScale` (config.rs): comprimentos de escala de tempestade ×0,25, ciclo de vida ×4 mais rápido,
base das nuvens desenhada a ×0,15 da altura física (máx. 190 blocos). O radar usa alcance físico =
alcance no mundo ÷ 0,25 para a altura do feixe, coerente com essa compressão. Tornados, danos e
detritos ficam em escala real.
