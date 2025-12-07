#!/bin/bash

# Register med custom=true
custom_registers=(61584 61590 61591 61582 61510 61511 60270 60271 60272 60273 60274 60275)

# Alla register
all_registers=(61513 61517 61534 61538 61546 61550 61554 61558 61562 61584 61590 61591 61521 61582 61510 61511 60270 60271 60272 60273 60274 60275)

# Map register IDs to human-readable names (from BMS Register manual)
declare -A register_names=(
  # Heating System 1 - Curve Settings
  [61513]="Heating_System_1_Curve_Inclination"
  [61517]="Heating_System_1_Curve_Adjustment"

  # Heating System 1 - Flow Temperature Limits
  [61534]="Heating_System_1_Max_Flow_Temp_°C"
  [61538]="Heating_System_1_Min_Flow_Temp_°C"

  # Heating System 1 - Heating Off Settings
  [61546]="Heating_System_1_Heating_Off_Outdoor_Temp_°C"
  [61550]="Heating_System_1_Heating_Off_Time"

  # Heating System 1 - Night Reduction
  [61554]="Heating_System_1_Room_Temp_Night_Reduction"
  [61558]="Heating_System_1_Flow_Night_Reduction"
  [61562]="Heating_System_1_Outdoor_Temp_Night_Reduction_Off"

  # Heat Pump Control
  [61521]="Heat_Pump_1_Blocked_Status"

  # Additional Heat / Electric Heater
  [61582]="Additional_Heat_Start_Degree_Minute"
  [61584]="Block_Additional_Heat_At_Outdoor_Temp_°C"
  [61590]="Max_Electric_Heater_Lower_Tank_kW"
  [61591]="Max_Electric_Heater_Upper_Tank_DHW_kW"

  # Other Heating Systems Room Temp
  [61510]="Heating_System_2_Room_Temp_Setting"
  [61511]="Heating_System_3_Room_Temp_Setting"

  # Validation Registers (Min/Max/Step)
  [60270]="Validation_Max_for_Reg_61590"
  [60271]="Validation_Min_for_Reg_61590"
  [60272]="Validation_Step_for_Reg_61590"
  [60273]="Validation_Max_for_Reg_61591"
  [60274]="Validation_Min_for_Reg_61591"
  [60275]="Validation_Step_for_Reg_61591"
)

api_url="http://192.168.10.19:3000/api/v1/ctc"

for addr in "${all_registers[@]}"; do
  if [[ " ${custom_registers[@]} " =~ " ${addr} " ]]; then
    url="${api_url}?addr=$addr&custom=true"
  else
    url="${api_url}?addr=$addr"
  fi

  value=$(curl -s "$url" | grep -oP '"ctc_data":\s*\K[0-9\.-]+')

  # Get the register name, or use the ID if not found
  name="${register_names[$addr]:-Register_$addr}"

  echo "$name ($addr): $value"
done
