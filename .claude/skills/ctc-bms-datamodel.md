# CTC BMS Data Model - Complete Register Reference

This skill provides the complete BMS register data model for CTC heating systems as documented in "Service document-BMS Register-17003548.pdf" (2021-11-08).

## When to Use This Skill

Use this skill when you need information about:
- Complete BMS register specifications and addresses
- Data types, scaling factors, and value ranges
- Register access rights (Read/Write)
- Visible register bit masking system
- Alarm and information text retrieval
- Single heat pump (A1) system configuration

**SCOPE**: This skill covers ALL registers for single heat pump systems. Multi-heatpump registers (A2-A10, heat pumps 2-10) are excluded as the user has only one heat pump.

**IMPORTANT**: This skill contains specifications from official documentation. Do NOT reference the Rust implementation - it may contain mistakes. Always trust the PDF documentation as the source of truth.

## Register Data Model Structure

### Register Table Columns

Each BMS register has the following attributes:

| Column | Description |
|--------|-------------|
| **Register** | Modbus register address (e.g., 61500, 62000) |
| **Description (Svenska)** | Swedish parameter description |
| **Description (English)** | English parameter description |
| **Signed** | 1 = Signed 16-bit integer, 0 = Unsigned 16-bit integer |
| **Read/Write** | R = Read-only, RW = Read/Write |
| **Max** | Register address containing maximum allowed value |
| **Min** | Register address containing minimum allowed value |
| **Step** | Register address containing step size value |
| **Visible** | Register address for visibility bit mask |
| **Bit** | Bit position in visible register (0-15) |
| **Factor** | Scaling factor to convert raw value to physical units |

### Data Types

**Signed (1)**
- 16-bit signed integer (S16)
- Range: -32768 to +32767
- Used for temperatures (can be negative), adjustments

**Unsigned (0)**
- 16-bit unsigned integer (U16)
- Range: 0 to 65535
- Used for counts, percentages, status codes

### Scaling Factors

Common scaling factors used throughout the system:

| Factor | Usage | Example |
|--------|-------|---------|
| **0.1** | Temperature values | Raw value 221 = 22.1°C |
| **1.0** | Counts, modes, integer values | Raw value 3 = 3 |
| **0.5** | Time in half-hours | Raw value 10 = 5 hours |

**Conversion formulas**:
- Physical value = Raw value × Factor
- Raw value = Physical value / Factor (rounded)

### Visible Register Bit Masking

The "Visible" register contains a bit field indicating which parameters are supported or active in the current system configuration.

**How it works**:
1. Read the visible register address (e.g., 62500)
2. Check if the bit at position "Bit" is set (1) or cleared (0)
3. If bit is set: parameter is visible/supported
4. If bit is cleared: parameter is not available in this system

**Example**:
- Register 61500 (Hot water mode) has Visible=62500, Bit=0
- Read register 62500, check bit 0
- If bit 0 is set: Hot water mode parameter is available

## Configuration Parameters (61000 Series)

**Important**: These registers use flash memory with limited write cycles. Minimize write operations.

### Hot Water Parameters

| Register | Description | Signed | R/W | Max | Min | Step | Visible | Bit | Factor |
|----------|-------------|--------|-----|-----|-----|------|---------|-----|--------|
| 61500 | Hot water mode<br>0=Economy, 1=Normal<br>2=Comfort, 3=Customized | 1 | RW | 60000 | 60001 | 60002 | 62500 | 0 | 1 |
| 61501 | Manual Stop temperature hot water | 1 | RW | 60003 | 60004 | 60005 | 62500 | 1 | 0.1 |
| 61502 | Setting outlet temperature hot water | 1 | RW | 60006 | 60007 | 60008 | 62500 | 2 | 0.1 |
| 61503 | Extra hot water timer | 1 | RW | 60009 | 60010 | 60011 | 62500 | 3 | 0.5 |
| 61504 | Maximum time heating Heat pump | 1 | RW | 60012 | 60013 | 60014 | 62500 | 4 | 1 |
| 61505 | Maximum time hot water | 1 | RW | 60015 | 60016 | 60017 | 62500 | 5 | 1 |
| 61506 | Minimum RPS hot water | 1 | RW | 60018 | 60019 | 60020 | 62500 | 6 | 0.1 |
| 61507 | Minimum RPS Pool | 1 | RW | 60021 | 60022 | 60023 | 62500 | 7 | 0.1 |

### General System Parameters

| Register | Description | Signed | R/W | Max | Min | Step | Visible | Bit | Factor |
|----------|-------------|--------|-----|-----|-----|------|---------|-----|--------|
| 61508 | Number of vacation days timer | 1 | RW | 60024 | 60025 | 60026 | 62500 | 8 | 1 |

### Heating System 1 Parameters

| Register | Description | Signed | R/W | Max | Min | Step | Visible | Bit | Factor |
|----------|-------------|--------|-----|-----|-----|------|---------|-----|--------|
| 61509 | Heating system 1: Setting room temp | 1 | RW | 60027 | 60028 | 60029 | 62500 | 9 | 0.1 |
| 61513 | Heating system 1: Change inclination | 1 | RW | 60039 | 60040 | 60041 | 62500 | 13 | 0.1 |
| 61517 | Heating system 1: Change adjustment | 1 | RW | 60051 | 60052 | 60053 | 62501 | 1 | 0.1 |
| 61534 | Heating system 1: Max Primary flow °C | 1 | RW | 60102 | 60103 | 60104 | 62502 | 2 | 0.1 |
| 61538 | Heating system 1: Min primary flow °C<br>140=Off | 1 | RW | 60114 | 60115 | 60116 | 62502 | 6 | 0.1 |
| 61542 | Heating system 1: Heating mode<br>0=Auto, 1=On, 2=Off | 1 | RW | 60126 | 60127 | 60128 | 62502 | 10 | 1 |
| 61546 | Heating system 1: Heating off, out °C | 1 | RW | 60138 | 60139 | 60140 | 62502 | 14 | 0.1 |
| 61550 | Heating system 1: Heating off time | 1 | RW | 60150 | 60151 | 60152 | 62503 | 2 | 1 |
| 61554 | Heating system 1: Room temp night reduction | 1 | RW | 60162 | 60163 | 60164 | 62503 | 6 | 0.1 |
| 61558 | Heating system 1: Primary flow Night reduction | 1 | RW | 60174 | 60175 | 60176 | 62503 | 10 | 0.1 |
| 61562 | Heating system 1: Outdoor temp night reduction | 1 | RW | 60186 | 60187 | 60188 | 62503 | 14 | 0.1 |
| 61566 | Heating system 1: Alarm low room temperature | 1 | RW | 60198 | 60199 | 60200 | 62504 | 2 | 0.1 |
| 61602 | Heating system 1: Holiday reduction | 1 | RW | 60306 | 60307 | 60308 | 62506 | 6 | 0.1 |
| 61606 | Heating system 1: Primary flow Holiday reduction | 1 | RW | 60318 | 60319 | 60320 | 62506 | 10 | 0.1 |

### Heating Systems 2-4 Parameters

**Note**: For systems with multiple heating circuits. Each heating system has identical parameter structure.

| Register Range | Description |
|----------------|-------------|
| 61510-61512 | Heating systems 2-4: Setting room temp |
| 61514-61516 | Heating systems 2-4: Change inclination |
| 61518-61520 | Heating systems 2-4: Change adjustment |
| 61535-61537 | Heating systems 2-4: Max Primary flow °C |
| 61539-61541 | Heating systems 2-4: Min primary flow °C (140=Off) |
| 61543-61545 | Heating systems 2-4: Heating mode (0=Auto, 1=On, 2=Off) |
| 61547-61549 | Heating systems 2-4: Heating off, out °C |
| 61551-61553 | Heating systems 2-4: Heating off time |
| 61555-61557 | Heating systems 2-4: Room temp night reduction |
| 61559-61561 | Heating systems 2-4: Primary flow Night reduction |
| 61563-61565 | Heating systems 2-4: Outdoor temp night reduction |
| 61567-61569 | Heating systems 2-4: Alarm low room temperature |
| 61603-61605 | Heating systems 2-4: Holiday reduction |
| 61607-61609 | Heating systems 2-4: Primary flow Holiday reduction |

### Heat Pump 1 (A1) Parameters

**Scope**: Single heat pump configuration only.

| Register | Description | Signed | R/W | Max | Min | Step | Visible | Bit | Factor |
|----------|-------------|--------|-----|-----|-----|------|---------|-----|--------|
| 61521 | Heat pump 1 (A1): Blocked<br>0=Blocked, 1=Allowed | 1 | RW | 60063 | 60064 | 60065 | 62501 | 5 | 1 |
| 61572 | Heat pump 1 (A1): Max RPS | 1 | RW | 60216 | 60217 | 60218 | 62504 | 8 | 0.1 |

### Pool Parameters

| Register | Description | Signed | R/W | Max | Min | Step | Visible | Bit | Factor |
|----------|-------------|--------|-----|-----|-----|------|---------|-----|--------|
| 61531 | Pool: Stop Temp setting | 1 | RW | 60093 | 60094 | 60095 | 62501 | 15 | 0.1 |
| 61532 | Pool: Maximum time | 1 | RW | 60096 | 60097 | 60098 | 62502 | 0 | 1 |
| 61533 | Pool: Start difference | 1 | RW | 60099 | 60100 | 60101 | 62502 | 1 | 0.1 |

### Radiator Pump and System Parameters

| Register | Description | Signed | R/W | Max | Min | Step | Visible | Bit | Factor |
|----------|-------------|--------|-----|-----|-----|------|---------|-----|--------|
| 61570 | Radiator pump setting % | 1 | RW | 60210 | 60211 | 60212 | 62504 | 6 | 1 |
| 61571 | Start at degree minute | 1 | RW | 60213 | 60214 | 60215 | 62504 | 7 | 1 |
| 61610 | Heat pump: Diff Heat pump, degree minute | 1 | RW | 60330 | 60331 | 60332 | 62506 | 14 | 1 |
| 61611 | Heat pump: Delay between Heat pump | 1 | RW | 60333 | 60334 | 60335 | 62506 | 15 | 1 |

### Additional Heat Source Parameters (E1, E2, E3)

**E1: Additional Heat**

| Register | Description | Signed | R/W | Max | Min | Step | Visible | Bit | Factor |
|----------|-------------|--------|-----|-----|-----|------|---------|-----|--------|
| 61582 | E1: Start add heat, degree minute | 1 | RW | 60246 | 60247 | 60248 | 62505 | 2 | 1 |
| 61612 | E1: Diff add heat, degree minute | 1 | RW | 60336 | 60337 | 60338 | 62507 | 0 | 1 |
| 61619 | E1: Delay add heat E1 | 1 | RW | 60357 | 60358 | 60359 | 62507 | 7 | 1 |

**E2: 0-10V Control**

| Register | Description | Signed | R/W | Max | Min | Step | Visible | Bit | Factor |
|----------|-------------|--------|-----|-----|-----|------|---------|-----|--------|
| 61613 | E2: Start 0-10V degree minute | 1 | RW | 60339 | 60340 | 60341 | 62507 | 1 | 1 |
| 61614 | E2: Diff 0-10V, degree minute | 1 | RW | 60342 | 60343 | 60344 | 62507 | 2 | 1 |
| 61620 | E2: Delay add heat 0-10V | 1 | RW | 60360 | 60361 | 60362 | 62507 | 8 | 1 |
| 61621 | E2: Diff 0-10V delay | 1 | RW | 60363 | 60364 | 60365 | 62507 | 9 | 1 |

**E3: EcoMiniEl**

| Register | Description | Signed | R/W | Max | Min | Step | Visible | Bit | Factor |
|----------|-------------|--------|-----|-----|-----|------|---------|-----|--------|
| 61615 | E3: Start EcoMiniEl, degree minute | 1 | RW | 60345 | 60346 | 60347 | 62507 | 3 | 1 |
| 61616 | E3: Number of steps heating | 1 | RW | 60348 | 60349 | 60350 | 62507 | 4 | 1 |
| 61617 | E3: Number of steps DHW | 1 | RW | 60351 | 60352 | 60353 | 62507 | 5 | 1 |
| 61618 | E3: Diff step EcoMiniEl | 1 | RW | 60354 | 60355 | 60356 | 62507 | 6 | 1 |
| 61622 | E3: Delay EcoMiniEl | 1 | RW | 60366 | 60367 | 60368 | 62507 | 10 | 1 |
| 61623 | E3: Delay EcoMiniEl step | 1 | RW | 60369 | 60370 | 60371 | 62507 | 11 | 1 |

### External Boiler Parameters

| Register | Description | Signed | R/W | Max | Min | Step | Visible | Bit | Factor |
|----------|-------------|--------|-----|-----|-----|------|---------|-----|--------|
| 61583 | External boiler diff | 1 | RW | 60249 | 60250 | 60251 | 62505 | 3 | 0.1 |
| 61584 | Blocking additional heat outdoor temp ºC | 1 | RW | 60252 | 60253 | 60254 | 62505 | 4 | 0.1 |
| 61585 | Boiler, open mixing valve ºC | 1 | RW | 60255 | 60256 | 60257 | 62505 | 5 | 0.1 |
| 61586 | Delay stop external boiler | 1 | RW | 60258 | 60259 | 60260 | 62505 | 6 | 1 |
| 61587 | External boiler mode<br>0=Auto, 1=On, 2=Off | 1 | RW | 60261 | 60262 | 60263 | 62505 | 7 | 1 |

### Electric Immersion Heater Parameters

| Register | Description | Signed | R/W | Max | Min | Step | Visible | Bit | Factor |
|----------|-------------|--------|-----|-----|-----|------|---------|-----|--------|
| 61590 | Max immersion heater kW / Lower | 1 | RW | 60270 | 60271 | 60272 | 62505 | 10 | 0.1 |
| 61591 | Max immersion heater DHW kW / Upper | 1 | RW | 60273 | 60274 | 60275 | 62505 | 11 | 0.1 |
| 61633 | Boiler lower °C | 1 | RW | 60399 | 60400 | 60401 | 62508 | 5 | 0.1 |
| 61634 | Boiler upper °C | 1 | RW | 60402 | 60403 | 60404 | 62508 | 6 | 0.1 |
| 61635 | Boiler add heat °C | 1 | RW | 60405 | 60406 | 60407 | 62508 | 7 | 0.1 |
| 61636 | Boiler DHW °C | 1 | RW | 60408 | 60409 | 60410 | 62508 | 8 | 0.1 |
| 61652 | Setpoint for upper tank when el. heater is used | 1 | RW | 60456 | 60457 | 60458 | 62509 | 8 | 0.1 |

### Free Cooling Parameters

| Register | Description | Signed | R/W | Max | Min | Step | Visible | Bit | Factor |
|----------|-------------|--------|-----|-----|-----|------|---------|-----|--------|
| 61624 | Cooling: Primary flow at outdoor temp +20 ºC | 1 | RW | 60372 | 60373 | 60374 | 62507 | 12 | 1 |
| 61625 | Cooling: Primary flow at outdoor temp +40 ºC | 1 | RW | 60375 | 60376 | 60377 | 62507 | 13 | 1 |
| 61626 | Cooling: Min flow temperature | 1 | RW | 60378 | 60379 | 60380 | 62507 | 14 | 1 |
| 61627 | Cooling: Start cooling at temperature | 1 | RW | 60381 | 60382 | 60383 | 62507 | 15 | 1 |
| 61628 | Cooling: Stop cooling at temperature | 1 | RW | 60384 | 60385 | 60386 | 62508 | 0 | 1 |

### EHS (Electric Heating System) Parameters

| Register | Description | Signed | R/W | Max | Min | Step | Visible | Bit | Factor |
|----------|-------------|--------|-----|-----|-----|------|---------|-----|--------|
| 61588 | EHS: Open mixing valve degrees | 1 | RW | 60264 | 60265 | 60266 | 62505 | 8 | 0.1 |
| 61589 | EHS: Start / stop diff | 1 | RW | 60267 | 60268 | 60269 | 62505 | 9 | 1 |

### Mixing Valve and System Control

| Register | Description | Signed | R/W | Max | Min | Step | Visible | Bit | Factor |
|----------|-------------|--------|-----|-----|-----|------|---------|-----|--------|
| 61629 | Delay mixing valve setting<br>241=Off | 1 | RW | 60387 | 60388 | 60389 | 62508 | 1 | 1 |

### Wood Boiler Parameters

| Register | Description | Signed | R/W | Max | Min | Step | Visible | Bit | Factor |
|----------|-------------|--------|-----|-----|-----|------|---------|-----|--------|
| 61630 | Wood boiler start flue gas<br>490=Off | 1 | RW | 60390 | 60391 | 60392 | 62508 | 2 | 0.1 |
| 61631 | Wood boiler start boiler temperature | 1 | RW | 60393 | 60394 | 60395 | 62508 | 3 | 0.1 |
| 61632 | Wood boiler hysteresis | 1 | RW | 60396 | 60397 | 60398 | 62508 | 4 | 0.1 |
| 61651 | Wood boiler buffer tank delay recharge time | 1 | RW | 60453 | 60454 | 60455 | 62509 | 7 | 1 |

### Differential Thermostat Parameters

| Register | Description | Signed | R/W | Max | Min | Step | Visible | Bit | Factor |
|----------|-------------|--------|-----|-----|-----|------|---------|-----|--------|
| 61637 | Diff thermostat start temp diff | 1 | RW | 60411 | 60412 | 60413 | 62508 | 9 | 0.1 |
| 61638 | Diff thermostat stop temp diff | 1 | RW | 60414 | 60415 | 60416 | 62508 | 10 | 0.1 |
| 61639 | Diff thermostat charge temperature | 1 | RW | 60417 | 60418 | 60419 | 62508 | 11 | 0.1 |

### Solar System Parameters

**Note**: Parameter functionality varies by product (EcoLogic/EcoZenith i550 vs. others)

| Register | Description | Signed | R/W | Max | Min | Step | Visible | Bit | Factor |
|----------|-------------|--------|-----|-----|-----|------|---------|-----|--------|
| 61640 | EcoLogic/EcoZenith i550: Solar deltaT max<br>Other: Solar borehole charge start diff | 1 | RW | 60420 | 60421 | 60422 | 62508 | 12 | 0.1 |
| 61641 | EcoLogic/EcoZenith i550: Solar deltaT min<br>Other: Solar borehole charge stop diff | 1 | RW | 60423 | 60424 | 60425 | 62508 | 13 | 0.1 |
| 61642 | Solar charge pump min | 1 | RW | 60426 | 60427 | 60428 | 62508 | 14 | 1 |
| 61643 | EcoLogic/EcoZenith i550: Solar deltaT max borehole<br>Other: Solar borehole charge start d | 1 | RW | 60429 | 60430 | 60431 | 62508 | 15 | 1 |
| 61644 | EcoLogic/EcoZenith i550: Solar deltaT min borehole<br>Other: Solar borehole charge stop dif | 1 | RW | 60432 | 60433 | 60434 | 62509 | 0 | 1 |
| 61645 | EcoHeat 400/EcoZenith i250: Solar H-tank charge temp<br>GSi/EcoZenith i350: Solar EHS-tan | 1 | RW | 60435 | 60436 | 60437 | 62509 | 1 | 1 |
| 61646 | Solar: X-tank charge temp | 1 | RW | 60438 | 60439 | 60440 | 62509 | 2 | 1 |
| 61647 | Solar: Eco-tank charge temp | 1 | RW | 60441 | 60442 | 60443 | 62509 | 3 | 1 |
| 61648 | EcoHeat 400/EcoZenith i250: Solar charge H-tank start diff<br>GSi/EcoZenith i350: So | 1 | RW | 60444 | 60445 | 60446 | 62509 | 4 | 1 |
| 61649 | EcoHeat 400/EcoZenith i250: Solar H-tank charge stop diff<br>GSi/EcoZenith i350: Sol | 1 | RW | 60447 | 60448 | 60449 | 62509 | 5 | 1 |
| 61650 | EcoHeat 400/EcoZenith i250: Solar H-tank charge stop temp<br>GSi/EcoZenith i350: S | 1 | RW | 60450 | 60451 | 60452 | 62509 | 6 | 1 |

### DHW Capacity and Tank Parameters

| Register | Description | Signed | R/W | Max | Min | Step | Visible | Bit | Factor |
|----------|-------------|--------|-----|-----|-----|------|---------|-----|--------|
| 61653 | Capacity start point for charging DHW | 1 | RW | 60459 | 60460 | 60461 | 62509 | 9 | 0.1 |
| 61654 | Lower temp sensor start point for charging DHW | 1 | RW | 60462 | 60463 | 60464 | 62509 | 10 | 0.1 |

### Ventilation Parameters

| Register | Description | Signed | R/W | Max | Min | Step | Visible | Bit | Factor |
|----------|-------------|--------|-----|-----|-----|------|---------|-----|--------|
| 61655 | Ventilation: Mode<br>0=Reduced, 1=Normal<br>2=Increased, 3=Manual | 1 | RW | 60465 | 60466 | 60467 | 62509 | 11 | 1 |
| 61656 | Turn night cooling on or off | 1 | RW | 60468 | 60469 | 60470 | 62509 | 12 | 1 |
| 61657 | Ventilation away mode | 1 | RW | 60471 | 60472 | 60473 | 62509 | 13 | 1 |

## Status and Reading Parameters (62000 Series)

**Important**: All registers in 62000 series are Read-Only.

### General System Status

| Register | Description | Signed | R/W | Visible | Bit | Factor |
|----------|-------------|--------|-----|---------|-----|--------|
| 62000 | Outdoor temperature | 1 | R | 62531 | 4 | 0.1 |
| 62001 | Stop temperature DHW | 1 | R | 62531 | 5 | 0.1 |
| 62002 | Setpoint outlet temperature DHW | 1 | R | 62531 | 6 | 1 |
| 62003 | Hot water temperature | 1 | R | 62531 | 7 | 0.1 |
| 62004 | Delay mixing valve | 1 | R | 62531 | 8 | 1 |
| 62005 | Status<br>**EcoHeat 400/EcoZenith 250**: 0=HP upper, 1=HP lower, 2=Add, 3=HP+Add<br>**GSi/EcoZenith 350**: 0=HS, 1=DHW, 2=Pool, 3=Off<br>**EcoZenith 550/EcoLogic**: 0=DHW, 1=HS, 2=Overtemp, 3=Ved, 4=DHW/HS, 5=Off | 1 | R | 62531 | 9 | 1 |
| 62006 | Radiator Water | 1 | R | 62531 | 10 | 0.1 |
| 62015 | Return temp | 1 | R | 62532 | 3 | 0.1 |
| 62016 | DHW circulation | 1 | R | 62532 | 4 | 1 |

### Heating System 1-4 Status

| Register | Description | Signed | R/W | Visible | Bit | Factor |
|----------|-------------|--------|-----|---------|-----|--------|
| 62007 | Heating system 1: Temperature setpoint primary flow | 1 | R | 62531 | 11 | 0.1 |
| 62008 | Heating system 2: Temperature setpoint primary flow | 1 | R | 62531 | 12 | 0.1 |
| 62009 | Heating system 3: Temperature setpoint primary flow | 1 | R | 62531 | 13 | 0.1 |
| 62010 | Heating system 4: Temperature setpoint primary flow | 1 | R | 62531 | 14 | 0.1 |
| 62011 | Heating system 1: Primary flow temperature | 1 | R | 62531 | 15 | 0.1 |
| 62012 | Heating system 2: Primary flow temperature | 1 | R | 62532 | 0 | 0.1 |
| 62013 | Heating system 3: Primary flow temperature | 1 | R | 62532 | 1 | 0.1 |
| 62014 | Heating system 4: Primary flow temperature | 1 | R | 62532 | 2 | 0.1 |
| 62246 | Heating system 1 status<br>0=Heating off, 1=Vacation<br>2=Night reduction, 3=On (normal) | 1 | R | 62546 | 10 | 1 |
| 62247 | Heating system 2 status<br>0=Heating off, 1=Vacation<br>2=Night reduction, 3=On (normal) | 1 | R | 62546 | 11 | 1 |
| 62248 | Heating system 3 status<br>0=Heating off, 1=Vacation<br>2=Night reduction, 3=On (normal) | 1 | R | 62546 | 12 | 1 |
| 62249 | Heating system 4 status<br>0=Heating off, 1=Vacation<br>2=Night reduction, 3=On (normal) | 1 | R | 62546 | 13 | 1 |

### Heat Pump 1 (A1) Status and Readings

**Status Register 62017**: Heat pump 1 (A1) Status

Status codes:
- 0 = Compressor_off_start_delay
- 1 = Compressor_off_ready_to_start
- 2 = Compressor_wait_until_flow
- 3 = Compressor_on_heating
- 4 = Defrost_active
- 5 = Compressor_on_cooling
- 6 = Compressor_off_blocked
- 7 = Compressor_off_alarm
- 8 = Function_test
- 30 = HP not defined
- 31 = Compressor not enabled
- 32 = Communication error
- 33 = Charge dhw

| Register | Description | Signed | R/W | Visible | Bit | Factor |
|----------|-------------|--------|-----|---------|-----|--------|
| 62017 | Heat pump 1 (A1): Status | 1 | R | 62532 | 5 | 1 |
| 62027 | Heat pump 1 (A1) HP in | 1 | R | 62532 | 15 | 0.1 |
| 62037 | Heat pump 1 (A1) HP out | 1 | R | 62533 | 9 | 0.1 |
| 62047 | Heat pump 1 (A1): Discharge temperature | 1 | R | 62534 | 3 | 0.1 |
| 62057 | Heat pump 1 (A1): Suction gas temperature | 1 | R | 62534 | 13 | 0.1 |
| 62067 | Heat pump 1 (A1): High pressure | 1 | R | 62535 | 7 | 0.1 |
| 62077 | Heat pump 1 (A1): Low Pressure | 1 | R | 62536 | 1 | 0.1 |
| 62087 | Heat pump 1 (A1): Brine in | 1 | R | 62536 | 11 | 0.1 |
| 62097 | Heat pump 1 (A1): Brine out | 1 | R | 62537 | 5 | 0.1 |
| 62107 | Heat pump 1 (A1): Charge pump | 1 | R | 62537 | 15 | 0.1 |
| 62117 | Heat pump 1 (A1): Brine pump | 1 | R | 62538 | 9 | 0.1 |
| 62127 | Heat pump 1 (A1): Fan | 1 | R | 62539 | 3 | 0.1 |
| 62137 | Heat pump 1 (A1): Defrost timer | 1 | R | 62539 | 13 | 1 |
| 62147 | Heat pump 1 (A1): Outdoor temp | 1 | R | 62540 | 7 | 0.1 |
| 62157 | Heat pump 1 (A1): Software version | 1 | R | 62541 | 1 | 1 |
| 62193 | Heat pump 1 (A1): Current RPS | 1 | R | 62543 | 5 | 0.1 |
| 62254 | Heat pump 1 (A1) Type | 1 | R | 62547 | 2 | 1 |
| 62264 | Heat pump 1 (A1) compressor model | 1 | R | 62547 | 12 | 1 |

### Room Temperature Readings

| Register | Description | Signed | R/W | Visible | Bit | Factor |
|----------|-------------|--------|-----|---------|-----|--------|
| 62203 | Current room temp 1 | 1 | R | 62543 | 15 | 0.1 |
| 62204 | Current room temp 2 | 1 | R | 62544 | 0 | 0.1 |
| 62205 | Current room temp 3 | 1 | R | 62544 | 1 | 0.1 |
| 62206 | Current room temp 4 | 1 | R | 62544 | 2 | 0.1 |

### System Operation Parameters

| Register | Description | Signed | R/W | Visible | Bit | Factor |
|----------|-------------|--------|-----|---------|-----|--------|
| 62167 | Degree minute | 1 | R | 62541 | 11 | 0.1 |
| 62168 | Power kW immersion heater | 1 | R | 62541 | 12 | 0.1 |
| 62169 | Power kW immersion heater lower | 1 | R | 62541 | 13 | 0.1 |
| 62170 | Maximum current | 1 | R | 62541 | 14 | 0.1 |
| 62171 | Current L1 | 1 | R | 62541 | 15 | 0.1 |
| 62172 | Current L2 | 1 | R | 62542 | 0 | 0.1 |
| 62173 | Current L3 | 1 | R | 62542 | 1 | 0.1 |
| 62174 | Pump Diff thermostat | 1 | R | 62542 | 2 | 1 |
| 62175 | Diff thermostat °C | 1 | R | 62542 | 3 | 0.1 |

### EHS (Electric Heating System) Status

| Register | Description | Signed | R/W | Visible | Bit | Factor |
|----------|-------------|--------|-----|---------|-----|--------|
| 62176 | EHS: Temperature | 1 | R | 62542 | 4 | 0.1 |
| 62177 | EHS: Primary flow -> Mode | 1 | R | 62542 | 5 | 1 |

### Pool Status

| Register | Description | Signed | R/W | Visible | Bit | Factor |
|----------|-------------|--------|-----|---------|-----|--------|
| 62178 | Pool Mode | 1 | R | 62542 | 6 | 1 |
| 62179 | Pool: Temperature | 1 | R | 62542 | 7 | 0.1 |
| 62180 | Pool: Stop temperature | 1 | R | 62542 | 8 | 0.1 |

### Solar System Status

| Register | Description | Signed | R/W | Visible | Bit | Factor |
|----------|-------------|--------|-----|---------|-----|--------|
| 62181 | Solar: Mode<br>0=Off, 1=On | 1 | R | 62542 | 9 | 1 |
| 62182 | Solar: Temperature out | 1 | R | 62542 | 10 | 0.1 |
| 62183 | Solar: Temperature in | 1 | R | 62542 | 11 | 0.1 |
| 62184 | Solar: Pump panel | 1 | R | 62542 | 12 | 1 |

### System Timers and Counters

| Register | Description | Signed | R/W | Visible | Bit | Factor |
|----------|-------------|--------|-----|---------|-----|--------|
| 62185 | Tank Timer | 1 | R | 62542 | 13 | 1 |
| 62186 | Stat: Total Operation LSB | 0 | R | 62542 | 14 | 1 |
| 62187 | Stat: Total Operation (<<16) MSB | - | - | - | - | - |
| 62191 | Stat: Immersion heater kWh | 1 | R | 62543 | 3 | 1 |
| 62192 | Function Test | 1 | R | 62543 | 4 | 1 |

### System Type and Version

| Register | Description | Signed | R/W | Visible | Bit | Factor |
|----------|-------------|--------|-----|---------|-----|--------|
| 62207 | System Type | 1 | R | 62544 | 3 | 1 |
| 62244 | Software version Display month day | 1 | R | 62546 | 8 | 1 |
| 62245 | Software version Display year | 1 | R | 62546 | 9 | 1 |
| 62253 | Product type | 1 | R | 62547 | 1 | 1 |

### Wood and Electric Boiler Status

| Register | Description | Signed | R/W | Visible | Bit | Factor |
|----------|-------------|--------|-----|---------|-----|--------|
| 62208 | Wood: Flue gas temperature (B8) | 1 | R | 62544 | 4 | 1 |
| 62209 | Wood: Temperature boiler (B9) | 1 | R | 62544 | 5 | 1 |
| 62210 | E1: Boiler temperature (B9) | 1 | R | 62544 | 6 | 0.1 |
| 62211 | E1: Temperature boiler out (B10) | 1 | R | 62544 | 7 | 0.1 |
| 62212 | E2: Number of steps | 1 | R | 62544 | 8 | 1 |
| 62213 | E3: Status | 1 | R | 62544 | 9 | 1 |

### Heat Pump Operating Time Statistics (LSB/MSB pairs)

**Note**: Operating time is stored in two registers (LSB and MSB) for 32-bit values.

| Register | Description | Signed | R/W | Visible | Bit | Factor |
|----------|-------------|--------|-----|---------|-----|--------|
| 62214 | Stat: Compressor 1 operating time LSB | 0 | R | 62544 | 10 | 1 |
| 62215 | Stat: Compressor 1 operating time (<<16) MSB | - | - | - | - | - |

### 24-Hour Heat Pump Statistics

| Register | Description | Signed | R/W | Visible | Bit | Factor |
|----------|-------------|--------|-----|---------|-----|--------|
| 62234 | Stat: Compressor 1 last 24h | 0 | R | 62545 | 14 | 1 |

### External Buffer Tanks

| Register | Description | Signed | R/W | Visible | Bit | Factor |
|----------|-------------|--------|-----|---------|-----|--------|
| 62250 | Ext buffer tank upper B41 | 1 | R | 62546 | 14 | 0.1 |
| 62251 | Ext buffer tank lower B42 | 1 | R | 62546 | 15 | 0.1 |
| 62252 | Ext DHW buffer tank B43 | 1 | R | 62547 | 0 | 0.1 |

### Tank Temperatures and Setpoints

| Register | Description | Signed | R/W | Visible | Bit | Factor |
|----------|-------------|--------|-----|---------|-----|--------|
| 62274 | Setpoint lower tank | 1 | R | 62548 | 6 | 0.1 |
| 62275 | Actual temperature DHW lower | 1 | R | 62548 | 7 | 0.1 |
| 62276 | Actual temperature DHW | 1 | R | 62548 | 8 | 0.1 |
| 62277 | Actual temperature tank solar coil | 1 | R | 62548 | 9 | 0.1 |
| 62278 | Calculated setpoint for upper tank when el. heater is used | 1 | R | 62548 | 10 | 0.1 |
| 62279 | Current DHW capacity in percent | 1 | R | 62548 | 11 | 0.1 |

### Ventilation Status

| Register | Description | Signed | R/W | Visible | Bit | Factor |
|----------|-------------|--------|-----|---------|-----|--------|
| 62280 | Exhaust fan speed percent | 1 | R | 62548 | 12 | 1 |
| 62281 | Highest measured CO2 | 1 | R | 62548 | 13 | 1 |
| 62282 | Highest measured humidity | 1 | R | 62548 | 14 | 1 |
| 62283 | Days until filter maintenance | 1 | R | 62548 | 15 | 1 |
| 62284 | Ventilation night cooling status | 1 | R | 62549 | 0 | 1 |

## Alarm and Information Text System (65000 Series)

The BMS provides a comprehensive alarm and information text retrieval system.

### System Registers

| Register | Description | Access | Notes |
|----------|-------------|--------|-------|
| **65000** | Read BMS version | R | Returns product ID:<br>0-999: EcoLogic<br>1000-1999: EZ550<br>2000-2999: EZ250<br>3000-3999: EH400<br>4000-4999: GSI<br>5000-5999: EZ350 |
| **65001** | Active alarms & info count | R/W | **Read**: Byte0=alarm count (max 255), Byte1=info count<br>**Write**: Reset alarms by writing 0xAA55 |
| **65002** | Extended information | R | Bit0: Alarm has been manually reset (bit cleared after read) |
| **65010-65059** | Alarm buffer (bit-coded) | R | 50 words, each bit represents alarm reference<br>Bit position = alarm reference number<br>If bit set: alarm is active |
| **65060-65069** | Info buffer (bit-coded) | R | 10 words, each bit represents info reference<br>Bit position = info reference number<br>If bit set: info is active |
| **65100** | Transfer alarm/info to text buffer | W | Write alarm/info reference to prepare text read:<br>0-9999: Alarm number 0-9999<br>10000-19999: Info number 0-9999 (write 10000+N) |
| **65101-65125** | ASCII text buffer | R | 25 words (50 bytes) of ASCII text<br>Format: [Heat pump][Number][Text]<br>Heat pump: A1-A10 (if applicable)<br>Number: Error/info reference in brackets<br>Text: Localized description |

### Alarm Detection and Reading Procedure

**Step-by-step process**:

1. **Poll for alarms** (every 5 seconds recommended)
   - Read register 65001
   - If non-zero: alarms or info texts are active
   - Example response: 0x0002 = 2 active alarms, 0 info texts

2. **Identify alarm references**
   - Read registers 65010-65059 (alarm buffer)
   - Each set bit indicates an active alarm
   - Bit position = alarm reference number
   - Example: Bit 16 set → alarm reference 16 is active

3. **Transfer alarm text to buffer**
   - Write alarm reference to register 65100
   - Note: Use zero-based indexing (bit 16 → write 15)
   - For info texts: write (10000 + reference number)

4. **Read alarm text**
   - Read registers 65101-65125
   - Each word contains 2 ASCII characters (big-endian)
   - Parse text format: [Heat pump][Error code][Description]

### Example: Detecting and Reading Alarm

**Scenario**: Two active alarms detected.

1. **Poll register 65001**
   ```
   Response: 0x0002
   → 2 active alarms, 0 info texts
   ```

2. **Read alarm buffer (65010-65059)**
   ```
   Response (hex):
   8000,0001,0000,0000,0000,0000,0000,0000,...

   Analysis:
   - Word 65010 = 0x8000 → Bit 15 set → Alarm reference 15
   - Word 65011 = 0x0001 → Bit 0 set → Alarm reference 16
   ```

3. **Transfer first alarm (reference 15) to buffer**
   ```
   Write 15 to register 65100
   ```

4. **Read text buffer (65101-65125)**
   ```
   Response (hex):
   5B45 3036 335D 204B 6F6D 6D75 6E69 6B2E 6665 6C20 7265 6C61 6B6F 7274 20...

   ASCII conversion:
   [E063] Kommunik.fel relakort

   Parse result:
   - No heat pump prefix (system-wide error)
   - Error code: E063
   - Description: "Kommunik.fel relakort" (Communication error relay card)
   ```

5. **Transfer second alarm (reference 16) to buffer**
   ```
   Write 16 to register 65100
   Read registers 65101-65125 for second alarm text
   ```

### Alarm Text Format

**Text structure**: `[Heat pump][Error code][Description]`

**Components**:
- **Heat pump**: A1-A10 (only present for heat pump-specific errors)
- **Error code**: Always in brackets, e.g., [E063]
- **Description**: Localized text depending on display language setting

**Examples**:
- `[E063] Kommunik.fel relakort` - System-wide communication error
- `A1[E015] Lågtryckspressostat` - Heat pump 1 low pressure switch alarm

### Alarm Reset Procedure

**To reset alarms**:
1. Write value 0xAA55 to register 65001
2. This resets manually clearable alarms
3. Alarms still active in hardware will re-trigger
4. Read register 65002 bit 0 to confirm manual reset occurred

## Best Practices

### Reading Registers

1. **Minimize Configuration Writes**
   - Registers 61000-61999 use flash memory
   - Limited write cycles (typically 10,000-100,000)
   - Only write when values actually change

2. **Respect Update Rates**
   - Minimum interval between reads: 1000ms
   - Remote control registers: update every 5 minutes maximum
   - Alarm polling: every 5 seconds recommended

3. **Batch Reads for Efficiency**
   - Read up to 100 registers per transaction
   - Group related registers together
   - Example: Read all heating system 1 temps in single call

4. **Check Visible Bits**
   - Before using a parameter, verify it's visible
   - Read the visible register, check the bit position
   - Prevents errors with unconfigured features

### Data Validation

1. **Signed Value Handling**
   - Check "Signed" column in register tables
   - For Signed=1: interpret as i16 (-32768 to +32767)
   - For Signed=0: interpret as u16 (0 to 65535)

2. **Apply Scaling Factors**
   - Always multiply raw value by factor
   - Physical value = Raw value × Factor
   - Example: Raw 221 with factor 0.1 = 22.1°C

3. **Validate Ranges**
   - Read Min/Max registers for writeable parameters
   - Check value is within bounds before writing
   - Apply step size constraints

### Multi-Heating System Support

The BMS supports up to 4 heating systems. Each heating system has identical parameter structure:

- **Heating System 1**: Primary heating circuit
- **Heating System 2-4**: Additional heating circuits (if installed)

**Parameter addressing pattern**:
- System 1: 61509, 61513, 61517, 61534...
- System 2: 61510, 61514, 61518, 61535...
- System 3: 61511, 61515, 61519, 61536...
- System 4: 61512, 61516, 61520, 61537...

## Document Reference

**Source**: Service document-BMS Register-17003548.pdf
**Date**: 2021-11-08
**Pages**: 3-15 (Complete register tables, alarm system, remote control)

## Notes

- This skill is based on OFFICIAL DOCUMENTATION ONLY
- The Rust implementation may contain errors - always trust the PDF
- Scope: Single heat pump (A1) systems only
- Multi-heatpump registers (A2-A10) are excluded per user requirements
- ALL other registers from the PDF are included in this specification
