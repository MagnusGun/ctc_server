# CTC EcoHeat 400 Configuration

This skill provides EcoHeat 400 specific configuration information extracted from "Service document-BMS Register-17003548.pdf" (2021-11-08).

## When to Use This Skill

Use this skill when you need information about:
- EcoHeat 400 specific settings and operational parameters
- Hot water (DHW) modes and temperature control
- Heating system configuration for EcoHeat 400
- Heat pump settings for single heat pump (A1) systems
- System modes, states, and operational ranges
- Tank configuration and solar integration specific to EcoHeat 400

**SCOPE**: This skill focuses on EcoHeat 400 with single heat pump configuration only.

**IMPORTANT**: This skill is based on official documentation. Do NOT reference the Rust implementation - it may contain mistakes. Always trust the PDF documentation.

## EcoHeat 400 Product Overview

**Product ID Range**: 3000-3999 (Register 65000)
**Compatible Systems**: Single or multiple heating circuits, single heat pump (A1)

### System Identification

To verify you're working with an EcoHeat 400:
- Read register 65000 (BMS version)
- Value between 3000-3999 indicates EcoHeat 400
- Read register 62253 (Product type) for specific variant

## Hot Water (DHW) System

### Hot Water Modes

**Register 61500**: Hot water mode (Factor: 1.0)
- **0 = Economy**: Lowest energy consumption, reduced comfort
- **1 = Normal**: Standard operation, balanced comfort/efficiency
- **2 = Comfort**: Maximum comfort, higher energy usage
- **3 = Customized**: Manual temperature control

**Configuration**:
- Access: Read/Write
- Max/Min/Step: Registers 60000/60001/60002
- Visible: Register 62500, Bit 0

### Hot Water Temperature Control

| Register | Parameter | Factor | Range | Description |
|----------|-----------|--------|-------|-------------|
| **61501** | Manual Stop temperature hot water | 0.1 | Check 60003-60005 | Temperature at which DHW heating stops in manual mode |
| **61502** | Setting outlet temperature hot water | 0.1 | Check 60006-60008 | Target DHW outlet temperature |
| **61505** | Maximum time hot water | 1.0 | Check 60015-60017 | Maximum duration for DHW heating cycle |
| **61506** | Minimum RPS hot water | 0.1 | Check 60018-60020 | Minimum heat pump speed for DHW |

**Status Registers**:
- **62001**: Stop temperature DHW (Read-only, Factor: 0.1)
- **62002**: Setpoint outlet temperature DHW (Read-only, Factor: 1.0)
- **62003**: Hot water temperature actual (Read-only, Factor: 0.1)

### Extra Hot Water Timer

**Register 61503**: Extra hot water timer (Factor: 0.5 hours)
- Extends DHW heating period
- Value in half-hour increments
- Example: Value 10 = 5 hours extra heating

### Tank Configuration (EcoHeat 400 Specific)

**Upper Tank Setpoint**:
- **Register 61652**: Setpoint for upper tank when el. heater is used (Factor: 0.1)
- **Register 62278**: Calculated setpoint for upper tank (Read-only, Factor: 0.1)

**Tank Status**:
- **Register 62274**: Setpoint lower tank (Read-only, Factor: 0.1)
- **Register 62275**: Actual temperature DHW lower (Read-only, Factor: 0.1)
- **Register 62276**: Actual temperature DHW (Read-only, Factor: 0.1)

**DHW Capacity**:
- **Register 61653**: Capacity start point for charging DHW (Factor: 0.1)
- **Register 61654**: Lower temp sensor start point for charging DHW (Factor: 0.1)
- **Register 62279**: Current DHW capacity in percent (Read-only, Factor: 0.1)

### Tank Timer

**Register 62185**: Tank Timer (Read-only, Factor: 1)
- Tracks DHW heating cycle timing

## Heating System Configuration

### Single Heating Circuit (Primary)

EcoHeat 400 supports single or multiple heating circuits. For single heat pump systems, focus on Heating System 1.

#### Room Temperature Control

**Register 61509**: Heating system 1: Setting room temperature
- Factor: 0.1 (e.g., 210 = 21.0°C)
- Access: Read/Write
- Min/Max/Step: Registers 60027/60028/60029
- Typical range: 15.0°C to 30.0°C

**Status**:
- **Register 62203**: Current room temp 1 (Read-only, Factor: 0.1)

#### Heating Curve Adjustment

**Register 61513**: Heating system 1: Change inclination
- Factor: 0.1
- Adjusts slope of heating curve
- Higher value = more aggressive heating response to outdoor temp

**Register 61517**: Heating system 1: Change adjustment
- Factor: 0.1
- Parallel shift of heating curve
- Positive value increases flow temperature across all outdoor temps

#### Flow Temperature Limits

**Register 61534**: Heating system 1: Max Primary flow °C
- Factor: 0.1
- Upper limit for primary flow temperature
- Protects system components

**Register 61538**: Heating system 1: Min primary flow °C
- Factor: 0.1
- Lower limit for primary flow temperature
- **Special value 140 (14.0°C) = Off**

**Status**:
- **Register 62007**: Heating system 1: Temperature setpoint primary flow (Read-only, Factor: 0.1)
- **Register 62011**: Heating system 1: Primary flow temperature (Read-only, Factor: 0.1)

#### Heating Mode

**Register 61542**: Heating system 1: Heating mode
- Factor: 1.0
- **0 = Auto**: Automatic control based on outdoor temperature
- **1 = On**: Force heating on
- **2 = Off**: Disable heating

#### Heating Off Conditions

**Register 61546**: Heating system 1: Heating off, out °C
- Factor: 0.1
- Outdoor temperature threshold to disable heating
- When outdoor temp exceeds this, heating turns off

**Register 61550**: Heating system 1: Heating off time
- Factor: 1.0
- Time duration in appropriate units

### Night Reduction (Energy Saving)

**Room Temperature Night Reduction**:
- **Register 61554**: Heating system 1: Room temp night reduction (Factor: 0.1)
- Reduces target room temperature during night hours

**Primary Flow Night Reduction**:
- **Register 61558**: Heating system 1: Primary flow Night reduction (Factor: 0.1)
- Reduces primary flow temperature during night hours

**Outdoor Temperature Threshold**:
- **Register 61562**: Heating system 1: Outdoor temp night reduction (Factor: 0.1)
- Night reduction only active when outdoor temp below this value

### Holiday/Vacation Mode

**Vacation Timer**:
- **Register 61508**: Number of vacation days timer (Factor: 1.0)
- Set number of days for vacation mode
- System operates in reduced mode

**Holiday Reduction Settings**:
- **Register 61602**: Heating system 1: Holiday reduction (Factor: 0.1)
- Room temperature reduction during vacation
- **Register 61606**: Heating system 1: Primary flow Holiday reduction (Factor: 0.1)
- Flow temperature reduction during vacation

**Status**:
- **Register 62246**: Heating system 1 status (Read-only, Factor: 1.0)
  - **0** = Heating off
  - **1** = Vacation mode active
  - **2** = Night reduction active
  - **3** = On (normal mode)

### Alarm Low Room Temperature

**Register 61566**: Heating system 1: Alarm low room temperature
- Factor: 0.1
- Triggers alarm if room temperature falls below this value
- Freeze protection feature

## Heat Pump Configuration (Single A1 Unit)

### Heat Pump Control

**Blocking Heat Pump**:
- **Register 61521**: Heat pump 1 (A1): Blocked
  - Factor: 1.0
  - **0** = Blocked (heat pump not allowed to run)
  - **1** = Allowed (heat pump can operate)

**Maximum Speed**:
- **Register 61572**: Heat pump 1 (A1): Max RPS (Revolutions Per Second)
  - Factor: 0.1
  - Limits maximum compressor speed
  - Lower value reduces capacity and noise

**Remote Control**:
- **Register 1002**: Maximum RPS (Remote control, update every 5 min)
- Allows external system to limit heat pump speed

### Heat Pump Status and Readings

**Status Register 62017**: Heat pump 1 (A1): Status (Factor: 1.0)

Status codes:
- **0**: Compressor off, start delay
- **1**: Compressor off, ready to start
- **2**: Compressor waiting for flow
- **3**: Compressor on, heating mode
- **4**: Defrost active
- **5**: Compressor on, cooling mode
- **6**: Compressor off, blocked
- **7**: Compressor off, alarm
- **8**: Function test running
- **30**: HP not defined in system
- **31**: Compressor not enabled
- **32**: Communication error
- **33**: Charging DHW

### Temperature Sensors

**Heat Pump Circuit**:
- **Register 62027**: Heat pump 1 (A1) HP in (Factor: 0.1)
- **Register 62037**: Heat pump 1 (A1) HP out (Factor: 0.1)

**Refrigerant Temperatures**:
- **Register 62047**: Heat pump 1 (A1): Discharge temperature (Factor: 0.1)
- **Register 62057**: Heat pump 1 (A1): Suction gas temperature (Factor: 0.1)

**Pressures**:
- **Register 62067**: Heat pump 1 (A1): High pressure (Factor: 0.1)
- **Register 62077**: Heat pump 1 (A1): Low Pressure (Factor: 0.1)

**Brine Circuit** (for ground source heat pumps):
- **Register 62087**: Heat pump 1 (A1): Brine in (Factor: 0.1)
- **Register 62097**: Heat pump 1 (A1): Brine out (Factor: 0.1)

### Pumps and Fan

**Pumps**:
- **Register 62107**: Heat pump 1 (A1): Charge pump (Factor: 0.1)
- **Register 62117**: Heat pump 1 (A1): Brine pump (Factor: 0.1)

**Fan**:
- **Register 62127**: Heat pump 1 (A1): Fan (Factor: 0.1)
- For air source heat pumps

### Defrost Operation

**Defrost Timer**:
- **Register 62137**: Heat pump 1 (A1): Defrost timer (Factor: 1.0)
- Counts down during defrost cycle
- Only applicable to air source heat pumps

### Heat Pump Information

**Outdoor Temperature**:
- **Register 62147**: Heat pump 1 (A1): Outdoor temp (Factor: 0.1)
- May differ from system outdoor temp sensor

**Current Speed**:
- **Register 62193**: Heat pump 1 (A1): Current RPS (Factor: 0.1)
- Actual compressor speed in revolutions per second

**Software and Type**:
- **Register 62157**: Heat pump 1 (A1): Software version (Factor: 1.0)
- **Register 62254**: Heat pump 1 (A1) Type (Factor: 1.0)
- **Register 62264**: Heat pump 1 (A1) compressor model (Factor: 1.0)

### Operating Time Statistics

**Total Operating Time**:
- **Register 62214**: Stat: Compressor 1 operating time LSB (Factor: 1.0)
- **Register 62215**: Stat: Compressor 1 operating time MSB
- Combine for 32-bit total hours

**24-Hour Statistics**:
- **Register 62234**: Stat: Compressor 1 last 24h (Factor: 1.0)
- Operating hours in last 24-hour period

## System Operation Status

### General System Status

**Register 62005**: Status (Read-only, Factor: 1.0)

**EcoHeat 400 Status Codes**:
- **0** = HP upper (Heat pump heating upper tank)
- **1** = HP lower (Heat pump heating lower tank)
- **2** = Add (Additional heat source active)
- **3** = HP + Add (Heat pump and additional heat both active)

### Outdoor Temperature

**Register 62000**: Outdoor temperature
- Factor: 0.1
- Primary outdoor temperature sensor
- Used for heating curve calculations

### Return Temperature

**Register 62015**: Return temp
- Factor: 0.1
- Return line temperature from heating circuit

### Radiator Water

**Register 62006**: Radiator Water
- Factor: 0.1
- Temperature in radiator circuit

### Radiator Pump

**Register 61570**: Radiator pump setting %
- Factor: 1.0
- Pump speed as percentage
- Range: typically 0-100%

## Degree Minute Control

### Degree Minute System

The degree minute system measures heating/cooling demand based on deviation from setpoint over time.

**Register 62167**: Degree minute (Read-only, Factor: 0.1)
- Cumulative measure of heating demand
- Positive value = heating needed
- Used to trigger additional heat sources

### Degree Minute Thresholds

**Register 61571**: Start at degree minute
- Factor: 1.0
- Threshold to start primary heating

**Heat Pump Control**:
- **Register 61610**: Heat pump: Diff Heat pump, degree minute (Factor: 1.0)
- **Register 61611**: Heat pump: Delay between Heat pump (Factor: 1.0)

**Additional Heat (E1)**:
- **Register 61582**: E1: Start add heat, degree minute (Factor: 1.0)
- Threshold to activate additional heat source
- **Register 61612**: E1: Diff add heat, degree minute (Factor: 1.0)

## Electric Immersion Heater

### Immersion Heater Configuration

**Maximum Power Limits**:
- **Register 61590**: Max immersion heater kW / Lower (Factor: 0.1)
- **Register 61591**: Max immersion heater DHW kW / Upper (Factor: 0.1)

**Remote Control Limits**:
- **Register 1003**: Maximum power immersion heater lower tank
- **Register 1004**: Maximum power immersion heater upper tank
- Update these at least every 5 minutes

**Temperature Setpoints**:
- **Register 61633**: Boiler lower °C (Factor: 0.1)
- **Register 61634**: Boiler upper °C (Factor: 0.1)
- **Register 61635**: Boiler add heat °C (Factor: 0.1)
- **Register 61636**: Boiler DHW °C (Factor: 0.1)

### Immersion Heater Status

**Current Power**:
- **Register 62168**: Power kW immersion heater (Read-only, Factor: 0.1)
- **Register 62169**: Power kW immersion heater lower (Read-only, Factor: 0.1)

**Current Consumption**:
- **Register 62171**: Current L1 (Factor: 0.1)
- **Register 62172**: Current L2 (Factor: 0.1)
- **Register 62173**: Current L3 (Factor: 0.1)
- **Register 62170**: Maximum current (Factor: 0.1)

**Energy Statistics**:
- **Register 62191**: Stat: Immersion heater kWh (Read-only, Factor: 1.0)
- Total energy consumed by immersion heater

## Solar Integration (EcoHeat 400)

EcoHeat 400 supports solar thermal integration with H-tank configuration.

### Solar System Status

**Register 62181**: Solar: Mode
- Factor: 1.0
- **0** = Off
- **1** = On (solar charging active)

**Temperatures**:
- **Register 62182**: Solar: Temperature out (Factor: 0.1)
- **Register 62183**: Solar: Temperature in (Factor: 0.1)
- **Register 62277**: Actual temperature tank solar coil (Factor: 0.1)

**Pump Status**:
- **Register 62184**: Solar: Pump panel (Factor: 1.0)

### Solar H-Tank Configuration (EcoHeat 400 Specific)

**Tank Charge Temperature**:
- **Register 61645**: EcoHeat 400: Solar H-tank charge temp
  - Factor: 1.0
  - Target temperature for solar charging of H-tank

**Charge Start/Stop Differentials**:
- **Register 61648**: EcoHeat 400: Solar charge H-tank start difference
  - Factor: 1.0
  - Temperature difference to start solar charging

- **Register 61649**: EcoHeat 400: Solar H-tank charge stop difference
  - Factor: 1.0
  - Temperature difference to stop solar charging

- **Register 61650**: EcoHeat 400: Solar H-tank charge stop temperature
  - Factor: 1.0
  - Absolute temperature to stop solar charging

### General Solar Parameters

**Pump Control**:
- **Register 61642**: Solar charge pump min (Factor: 1.0)
- Minimum pump speed

**X-Tank and Eco-Tank**:
- **Register 61646**: Solar: X-tank charge temp (Factor: 1.0)
- **Register 61647**: Solar: Eco-tank charge temp (Factor: 1.0)

**Differential Thermostat**:
- **Register 61637**: Diff thermostat start temp diff (Factor: 0.1)
- **Register 61638**: Diff thermostat stop temp diff (Factor: 0.1)
- **Register 61639**: Diff thermostat charge temperature (Factor: 0.1)

**Status Readings**:
- **Register 62174**: Pump Diff thermostat (Read-only, Factor: 1.0)
- **Register 62175**: Diff thermostat °C (Read-only, Factor: 0.1)

## Pool Heating (Optional)

EcoHeat 400 can support pool heating when configured.

### Pool Configuration

**Temperature Control**:
- **Register 61531**: Pool: Stop Temp setting (Factor: 0.1)
- **Register 61532**: Pool: Maximum time (Factor: 1.0)
- **Register 61533**: Pool: Start difference (Factor: 0.1)

**Minimum Speed**:
- **Register 61507**: Minimum RPS Pool (Factor: 0.1)

### Pool Status

**Register 62178**: Pool Mode (Read-only, Factor: 1.0)
- Current pool heating mode

**Temperatures**:
- **Register 62179**: Pool: Temperature (Read-only, Factor: 0.1)
- **Register 62180**: Pool: Stop temperature (Read-only, Factor: 0.1)

## System Timing and Control

### Maximum Heating Times

**Register 61504**: Maximum time heating Heat pump
- Factor: 1.0
- Maximum continuous heating time
- Prevents excessive heat pump runtime

### Total System Operation

**Register 62186**: Stat: Total Operation LSB (Factor: 1.0)
**Register 62187**: Stat: Total Operation MSB
- Combined 32-bit value
- Total system operating hours

## Operational Ranges and Constraints

### Typical Temperature Ranges (EcoHeat 400)

**Room Temperature**:
- Setpoint range: 15.0°C - 30.0°C
- Alarm low: typically 5.0°C - 15.0°C

**Primary Flow Temperature**:
- Min: 14.0°C (or OFF if set to 140)
- Max: typically 55°C - 75°C (depends on system)
- Setpoint calculated from heating curve

**DHW Temperature**:
- Stop temperature: typically 45°C - 65°C
- Outlet temperature: typically 50°C - 60°C
- Tank temperatures: 40°C - 70°C

**Outdoor Temperature**:
- Typical measurement range: -30°C to +40°C
- Heating off threshold: typically +15°C to +22°C

### Heat Pump Operational Ranges

**RPS (Revolutions Per Second)**:
- Minimum RPS: typically 15-30 RPS
- Maximum RPS: typically 80-120 RPS (model dependent)
- Values scaled by factor 0.1

**Pressures** (model dependent):
- High pressure: typically 5-35 bar
- Low pressure: typically -1 to 8 bar

## Configuration Best Practices

### Initial Setup

1. **Verify System Identification**
   - Read register 65000 (should be 3000-3999)
   - Read register 62253 (product type)
   - Confirm heat pump type from register 62254

2. **Configure Hot Water**
   - Set DHW mode (register 61500) based on comfort needs
   - Set outlet temperature (register 61502)
   - Configure max heating time (register 61505)

3. **Configure Heating System**
   - Set desired room temperature (register 61509)
   - Adjust heating curve if needed (registers 61513, 61517)
   - Set flow temperature limits (registers 61534, 61538)

4. **Configure Energy Saving**
   - Enable night reduction if desired (registers 61554, 61558, 61562)
   - Configure vacation mode defaults (registers 61602, 61606)

5. **Configure Heat Pump**
   - Set max RPS appropriate for system (register 61572)
   - Verify heat pump is not blocked (register 61521)

### Monitoring

**Essential Status Registers**:
- System status: 62005
- Heat pump status: 62017
- Heating system status: 62246
- Room temperature: 62203
- Outdoor temperature: 62000
- Primary flow temperature: 62011
- DHW temperature: 62276

**Poll Frequency**:
- Critical parameters: every 5-10 seconds
- Temperature readings: every 30-60 seconds
- Statistics: every 5-60 minutes
- Alarms: every 5 seconds (register 65001)

### Energy Optimization

1. **Use Degree Minute Control**
   - Monitor register 62167
   - Adjust thresholds (61571, 61582) for optimal staging

2. **Night Reduction**
   - Configure appropriate reduction values
   - Set outdoor temperature threshold

3. **Vacation Mode**
   - Set vacation days before leaving
   - System automatically reduces operation

4. **Heat Pump Speed Limiting**
   - Reduce max RPS during off-peak demand
   - Use remote control register 1002 for dynamic control

## Document Reference

**Source**: Service document-BMS Register-17003548.pdf
**Date**: 2021-11-08
**Product ID**: 3000-3999 (EcoHeat 400)

## Notes

- This skill is based on OFFICIAL DOCUMENTATION ONLY
- The Rust implementation may contain errors - always trust the PDF
- Scope: EcoHeat 400 with single heat pump (A1) configuration
- For multi-heatpump systems, refer to the complete BMS data model skill
- All register addresses and specifications are from the official service document
