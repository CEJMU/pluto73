
################################################################
# This is a generated script based on design: system
#
# Though there are limitations about the generated script,
# the main purpose of this utility is to make learning
# IP Integrator Tcl commands easier.
################################################################

namespace eval _tcl {
proc get_script_folder {} {
   set script_path [file normalize [info script]]
   set script_folder [file dirname $script_path]
   return $script_folder
}
}
variable script_folder
set script_folder [_tcl::get_script_folder]

################################################################
# Check if script is running in correct Vivado version.
################################################################
set scripts_vivado_version 2022.2
set current_vivado_version [version -short]

if { [string first $scripts_vivado_version $current_vivado_version] == -1 } {
   puts ""
   catch {common::send_gid_msg -ssname BD::TCL -id 2041 -severity "ERROR" "This script was generated using Vivado <$scripts_vivado_version> and is being run in <$current_vivado_version> of Vivado. Please run the script in Vivado <$scripts_vivado_version> then open the design in Vivado <$current_vivado_version>. Upgrade the design by running \"Tools => Report => Report IP Status...\", then run write_bd_tcl to create an updated script."}

   return 1
}

################################################################
# START
################################################################

# To test this script, run the following commands from Vivado Tcl console:
# source system_script.tcl


# The design that will be created by this Tcl script contains the following 
# module references:
# burst_gate, dsp_mux_rx, dsp_mux_tx, iq_packer, tx_strobe_gen

# Please add the sources of those modules before sourcing this Tcl script.

# If there is no project opened, this script will create a
# project, but make sure you do not have an existing project
# <./myproj/project_1.xpr> in the current working folder.

set list_projs [get_projects -quiet]
if { $list_projs eq "" } {
   create_project project_1 myproj -part xc7z010clg400-1
}


# CHANGE DESIGN NAME HERE
variable design_name
set design_name system

# If you do not already have an existing IP Integrator design open,
# you can create a design using the following command:
#    create_bd_design $design_name

# Creating design if needed
set errMsg ""
set nRet 0

set cur_design [current_bd_design -quiet]
set list_cells [get_bd_cells -quiet]

if { ${design_name} eq "" } {
   # USE CASES:
   #    1) Design_name not set

   set errMsg "Please set the variable <design_name> to a non-empty value."
   set nRet 1

} elseif { ${cur_design} ne "" && ${list_cells} eq "" } {
   # USE CASES:
   #    2): Current design opened AND is empty AND names same.
   #    3): Current design opened AND is empty AND names diff; design_name NOT in project.
   #    4): Current design opened AND is empty AND names diff; design_name exists in project.

   if { $cur_design ne $design_name } {
      common::send_gid_msg -ssname BD::TCL -id 2001 -severity "INFO" "Changing value of <design_name> from <$design_name> to <$cur_design> since current design is empty."
      set design_name [get_property NAME $cur_design]
   }
   common::send_gid_msg -ssname BD::TCL -id 2002 -severity "INFO" "Constructing design in IPI design <$cur_design>..."

} elseif { ${cur_design} ne "" && $list_cells ne "" && $cur_design eq $design_name } {
   # USE CASES:
   #    5) Current design opened AND has components AND same names.

   set errMsg "Design <$design_name> already exists in your project, please set the variable <design_name> to another value."
   set nRet 1
} elseif { [get_files -quiet ${design_name}.bd] ne "" } {
   # USE CASES: 
   #    6) Current opened design, has components, but diff names, design_name exists in project.
   #    7) No opened design, design_name exists in project.

   set errMsg "Design <$design_name> already exists in your project, please set the variable <design_name> to another value."
   set nRet 2

} else {
   # USE CASES:
   #    8) No opened design, design_name not in project.
   #    9) Current opened design, has components, but diff names, design_name not in project.

   common::send_gid_msg -ssname BD::TCL -id 2003 -severity "INFO" "Currently there is no design <$design_name> in project, so creating one..."

   create_bd_design $design_name

   common::send_gid_msg -ssname BD::TCL -id 2004 -severity "INFO" "Making design <$design_name> as current_bd_design."
   current_bd_design $design_name

}

common::send_gid_msg -ssname BD::TCL -id 2005 -severity "INFO" "Currently the variable <design_name> is equal to \"$design_name\"."

if { $nRet != 0 } {
   catch {common::send_gid_msg -ssname BD::TCL -id 2006 -severity "ERROR" $errMsg}
   return $nRet
}

set bCheckIPsPassed 1
##################################################################
# CHECK IPs
##################################################################
set bCheckIPs 1
if { $bCheckIPs == 1 } {
   set list_check_ips "\ 
xilinx.com:ip:xlconstant:1.1\
xilinx.com:ip:util_vector_logic:2.0\
analog.com:user:axi_ad9361:1.0\
analog.com:user:axi_dmac:1.0\
xilinx.com:ip:axi_gpio:2.0\
xilinx.com:ip:axi_iic:2.1\
xilinx.com:ip:axi_quad_spi:3.2\
xilinx.com:ip:xlslice:1.0\
analog.com:user:util_cpack2:1.0\
xilinx.com:ip:proc_sys_reset:5.0\
xilinx.com:ip:cic_compiler:4.0\
xilinx.com:ip:cmpy:6.0\
xilinx.com:ip:axis_data_fifo:2.0\
xilinx.com:ip:dds_compiler:6.0\
xilinx.com:ip:fir_compiler:7.2\
xilinx.com:ip:xlconcat:2.1\
xilinx.com:ip:processing_system7:5.5\
analog.com:user:util_upack2:1.0\
analog.com:user:axi_tdd:1.0\
"

   set list_ips_missing ""
   common::send_gid_msg -ssname BD::TCL -id 2011 -severity "INFO" "Checking if the following IPs exist in the project's IP catalog: $list_check_ips ."

   foreach ip_vlnv $list_check_ips {
      set ip_obj [get_ipdefs -all $ip_vlnv]
      if { $ip_obj eq "" } {
         lappend list_ips_missing $ip_vlnv
      }
   }

   if { $list_ips_missing ne "" } {
      catch {common::send_gid_msg -ssname BD::TCL -id 2012 -severity "ERROR" "The following IPs are not found in the IP Catalog:\n  $list_ips_missing\n\nResolution: Please add the repository containing the IP(s) to the project." }
      set bCheckIPsPassed 0
   }

}

##################################################################
# CHECK Modules
##################################################################
set bCheckModules 1
if { $bCheckModules == 1 } {
   set list_check_mods "\ 
burst_gate\
dsp_mux_rx\
dsp_mux_tx\
iq_packer\
tx_strobe_gen\
"

   set list_mods_missing ""
   common::send_gid_msg -ssname BD::TCL -id 2020 -severity "INFO" "Checking if the following modules exist in the project's sources: $list_check_mods ."

   foreach mod_vlnv $list_check_mods {
      if { [can_resolve_reference $mod_vlnv] == 0 } {
         lappend list_mods_missing $mod_vlnv
      }
   }

   if { $list_mods_missing ne "" } {
      catch {common::send_gid_msg -ssname BD::TCL -id 2021 -severity "ERROR" "The following module(s) are not found in the project: $list_mods_missing" }
      common::send_gid_msg -ssname BD::TCL -id 2022 -severity "INFO" "Please add source files for the missing module(s) above."
      set bCheckIPsPassed 0
   }
}

if { $bCheckIPsPassed != 1 } {
  common::send_gid_msg -ssname BD::TCL -id 2023 -severity "WARNING" "Will not continue with creation of design due to the error(s) above."
  return 3
}

##################################################################
# DESIGN PROCs
##################################################################


# Hierarchical cell: axi_tdd_0
proc create_hier_cell_axi_tdd_0 { parentCell nameHier } {

  variable script_folder

  if { $parentCell eq "" || $nameHier eq "" } {
     catch {common::send_gid_msg -ssname BD::TCL -id 2092 -severity "ERROR" "create_hier_cell_axi_tdd_0() - Empty argument(s)!"}
     return
  }

  # Get object for parentCell
  set parentObj [get_bd_cells $parentCell]
  if { $parentObj == "" } {
     catch {common::send_gid_msg -ssname BD::TCL -id 2090 -severity "ERROR" "Unable to find parent cell <$parentCell>!"}
     return
  }

  # Make sure parentObj is hier blk
  set parentType [get_property TYPE $parentObj]
  if { $parentType ne "hier" } {
     catch {common::send_gid_msg -ssname BD::TCL -id 2091 -severity "ERROR" "Parent <$parentObj> has TYPE = <$parentType>. Expected to be <hier>."}
     return
  }

  # Save current instance; Restore later
  set oldCurInst [current_bd_instance .]

  # Set parent object as current
  current_bd_instance $parentObj

  # Create cell and set as current instance
  set hier_obj [create_bd_cell -type hier $nameHier]
  current_bd_instance $hier_obj

  # Create interface pins
  create_bd_intf_pin -mode Slave -vlnv xilinx.com:interface:aximm_rtl:1.0 s_axi


  # Create pins
  create_bd_pin -dir I -type clk clk
  create_bd_pin -dir I -type rst resetn
  create_bd_pin -dir I -type clk s_axi_aclk
  create_bd_pin -dir I -type rst s_axi_aresetn
  create_bd_pin -dir I sync_in
  create_bd_pin -dir O sync_out
  create_bd_pin -dir O -from 0 -to 0 tdd_channel_0
  create_bd_pin -dir O -from 0 -to 0 tdd_channel_1
  create_bd_pin -dir O -from 0 -to 0 tdd_channel_2

  # Create instance: tdd_ch_slice_0, and set properties
  set tdd_ch_slice_0 [ create_bd_cell -type ip -vlnv xilinx.com:ip:xlslice:1.0 tdd_ch_slice_0 ]
  set_property -dict [list \
    CONFIG.DIN_FROM {0} \
    CONFIG.DIN_TO {0} \
    CONFIG.DIN_WIDTH {3} \
  ] $tdd_ch_slice_0


  # Create instance: tdd_ch_slice_1, and set properties
  set tdd_ch_slice_1 [ create_bd_cell -type ip -vlnv xilinx.com:ip:xlslice:1.0 tdd_ch_slice_1 ]
  set_property -dict [list \
    CONFIG.DIN_FROM {1} \
    CONFIG.DIN_TO {1} \
    CONFIG.DIN_WIDTH {3} \
  ] $tdd_ch_slice_1


  # Create instance: tdd_ch_slice_2, and set properties
  set tdd_ch_slice_2 [ create_bd_cell -type ip -vlnv xilinx.com:ip:xlslice:1.0 tdd_ch_slice_2 ]
  set_property -dict [list \
    CONFIG.DIN_FROM {2} \
    CONFIG.DIN_TO {2} \
    CONFIG.DIN_WIDTH {3} \
  ] $tdd_ch_slice_2


  # Create instance: tdd_core, and set properties
  set tdd_core [ create_bd_cell -type ip -vlnv analog.com:user:axi_tdd:1.0 tdd_core ]
  set_property -dict [list \
    CONFIG.BURST_COUNT_WIDTH {32} \
    CONFIG.CHANNEL_COUNT {3} \
    CONFIG.DEFAULT_POLARITY {0b00000010} \
    CONFIG.REGISTER_WIDTH {32} \
    CONFIG.SYNC_COUNT_WIDTH {0} \
    CONFIG.SYNC_EXTERNAL {1} \
    CONFIG.SYNC_EXTERNAL_CDC {1} \
    CONFIG.SYNC_INTERNAL {0} \
  ] $tdd_core


  # Create interface connections
  connect_bd_intf_net -intf_net s_axi_1 [get_bd_intf_pins s_axi] [get_bd_intf_pins tdd_core/s_axi]

  # Create port connections
  connect_bd_net -net clk_1 [get_bd_pins clk] [get_bd_pins tdd_core/clk]
  connect_bd_net -net resetn_1 [get_bd_pins resetn] [get_bd_pins tdd_core/resetn]
  connect_bd_net -net s_axi_aclk_1 [get_bd_pins s_axi_aclk] [get_bd_pins tdd_core/s_axi_aclk]
  connect_bd_net -net s_axi_aresetn_1 [get_bd_pins s_axi_aresetn] [get_bd_pins tdd_core/s_axi_aresetn]
  connect_bd_net -net sync_in_1 [get_bd_pins sync_in] [get_bd_pins tdd_core/sync_in]
  connect_bd_net -net tdd_ch_slice_0_Dout [get_bd_pins tdd_channel_0] [get_bd_pins tdd_ch_slice_0/Dout]
  connect_bd_net -net tdd_ch_slice_1_Dout [get_bd_pins tdd_channel_1] [get_bd_pins tdd_ch_slice_1/Dout]
  connect_bd_net -net tdd_ch_slice_2_Dout [get_bd_pins tdd_channel_2] [get_bd_pins tdd_ch_slice_2/Dout]
  connect_bd_net -net tdd_core_sync_out [get_bd_pins sync_out] [get_bd_pins tdd_core/sync_out]
  connect_bd_net -net tdd_core_tdd_channel [get_bd_pins tdd_ch_slice_0/Din] [get_bd_pins tdd_ch_slice_1/Din] [get_bd_pins tdd_ch_slice_2/Din] [get_bd_pins tdd_core/tdd_channel]

  # Restore current instance
  current_bd_instance $oldCurInst
}


# Procedure to create entire design; Provide argument to make
# procedure reusable. If parentCell is "", will use root.
proc create_root_design { parentCell } {

  variable script_folder
  variable design_name

  if { $parentCell eq "" } {
     set parentCell [get_bd_cells /]
  }

  # Get object for parentCell
  set parentObj [get_bd_cells $parentCell]
  if { $parentObj == "" } {
     catch {common::send_gid_msg -ssname BD::TCL -id 2090 -severity "ERROR" "Unable to find parent cell <$parentCell>!"}
     return
  }

  # Make sure parentObj is hier blk
  set parentType [get_property TYPE $parentObj]
  if { $parentType ne "hier" } {
     catch {common::send_gid_msg -ssname BD::TCL -id 2091 -severity "ERROR" "Parent <$parentObj> has TYPE = <$parentType>. Expected to be <hier>."}
     return
  }

  # Save current instance; Restore later
  set oldCurInst [current_bd_instance .]

  # Set parent object as current
  current_bd_instance $parentObj


  # Create interface ports
  set ddr [ create_bd_intf_port -mode Master -vlnv xilinx.com:interface:ddrx_rtl:1.0 ddr ]

  set fixed_io [ create_bd_intf_port -mode Master -vlnv xilinx.com:display_processing_system7:fixedio_rtl:1.0 fixed_io ]

  set iic_main [ create_bd_intf_port -mode Master -vlnv xilinx.com:interface:iic_rtl:1.0 iic_main ]


  # Create ports
  set enable [ create_bd_port -dir O enable ]
  set gpio_i [ create_bd_port -dir I -from 17 -to 0 gpio_i ]
  set gpio_o [ create_bd_port -dir O -from 17 -to 0 gpio_o ]
  set gpio_t [ create_bd_port -dir O -from 17 -to 0 gpio_t ]
  set rx_clk_in [ create_bd_port -dir I rx_clk_in ]
  set rx_data_in [ create_bd_port -dir I -from 11 -to 0 rx_data_in ]
  set rx_frame_in [ create_bd_port -dir I rx_frame_in ]
  set spi0_clk_i [ create_bd_port -dir I spi0_clk_i ]
  set spi0_clk_o [ create_bd_port -dir O spi0_clk_o ]
  set spi0_csn_0_o [ create_bd_port -dir O spi0_csn_0_o ]
  set spi0_csn_1_o [ create_bd_port -dir O spi0_csn_1_o ]
  set spi0_csn_2_o [ create_bd_port -dir O spi0_csn_2_o ]
  set spi0_csn_i [ create_bd_port -dir I spi0_csn_i ]
  set spi0_sdi_i [ create_bd_port -dir I spi0_sdi_i ]
  set spi0_sdo_i [ create_bd_port -dir I spi0_sdo_i ]
  set spi0_sdo_o [ create_bd_port -dir O spi0_sdo_o ]
  set spi_clk_i [ create_bd_port -dir I spi_clk_i ]
  set spi_clk_o [ create_bd_port -dir O spi_clk_o ]
  set spi_csn_i [ create_bd_port -dir I spi_csn_i ]
  set spi_csn_o [ create_bd_port -dir O -from 0 -to 0 spi_csn_o ]
  set spi_sdi_i [ create_bd_port -dir I spi_sdi_i ]
  set spi_sdo_i [ create_bd_port -dir I spi_sdo_i ]
  set spi_sdo_o [ create_bd_port -dir O spi_sdo_o ]
  set tdd_ext_sync [ create_bd_port -dir I tdd_ext_sync ]
  set tx_clk_out [ create_bd_port -dir O tx_clk_out ]
  set tx_data_out [ create_bd_port -dir O -from 11 -to 0 tx_data_out ]
  set tx_frame_out [ create_bd_port -dir O tx_frame_out ]
  set txdata_o [ create_bd_port -dir O -from 0 -to 0 txdata_o ]
  set txnrx [ create_bd_port -dir O txnrx ]
  set up_enable [ create_bd_port -dir I up_enable ]
  set up_txnrx [ create_bd_port -dir I up_txnrx ]

  # Create instance: GND_1, and set properties
  set GND_1 [ create_bd_cell -type ip -vlnv xilinx.com:ip:xlconstant:1.1 GND_1 ]
  set_property -dict [list \
    CONFIG.CONST_VAL {0} \
    CONFIG.CONST_WIDTH {1} \
  ] $GND_1


  # Create instance: ad9361_resetn, and set properties
  set ad9361_resetn [ create_bd_cell -type ip -vlnv xilinx.com:ip:util_vector_logic:2.0 ad9361_resetn ]
  set_property -dict [list \
    CONFIG.C_OPERATION {not} \
    CONFIG.C_SIZE {1} \
  ] $ad9361_resetn


  # Create instance: axi_ad9361, and set properties
  set axi_ad9361 [ create_bd_cell -type ip -vlnv analog.com:user:axi_ad9361:1.0 axi_ad9361 ]
  set_property -dict [list \
    CONFIG.ADC_INIT_DELAY {21} \
    CONFIG.CMOS_OR_LVDS_N {1} \
    CONFIG.ID {0} \
    CONFIG.MODE_1R1T {0} \
  ] $axi_ad9361


  # Create instance: axi_ad9361_adc_dma, and set properties
  set axi_ad9361_adc_dma [ create_bd_cell -type ip -vlnv analog.com:user:axi_dmac:1.0 axi_ad9361_adc_dma ]
  set_property -dict [list \
    CONFIG.AXI_SLICE_DEST {false} \
    CONFIG.AXI_SLICE_SRC {false} \
    CONFIG.CYCLIC {false} \
    CONFIG.DISABLE_DEBUG_REGISTERS {true} \
    CONFIG.DMA_2D_TRANSFER {false} \
    CONFIG.DMA_DATA_WIDTH_SRC {64} \
    CONFIG.DMA_TYPE_DEST {0} \
    CONFIG.DMA_TYPE_SRC {2} \
    CONFIG.SYNC_TRANSFER_START {true} \
  ] $axi_ad9361_adc_dma


  # Create instance: axi_ad9361_dac_dma, and set properties
  set axi_ad9361_dac_dma [ create_bd_cell -type ip -vlnv analog.com:user:axi_dmac:1.0 axi_ad9361_dac_dma ]
  set_property -dict [list \
    CONFIG.AXI_SLICE_DEST {false} \
    CONFIG.AXI_SLICE_SRC {false} \
    CONFIG.CYCLIC {true} \
    CONFIG.DISABLE_DEBUG_REGISTERS {true} \
    CONFIG.DMA_2D_TRANSFER {false} \
    CONFIG.DMA_DATA_WIDTH_DEST {64} \
    CONFIG.DMA_TYPE_DEST {1} \
    CONFIG.DMA_TYPE_SRC {0} \
  ] $axi_ad9361_dac_dma


  # Create instance: axi_cpu_interconnect, and set properties
  set axi_cpu_interconnect [ create_bd_cell -type ip -vlnv xilinx.com:ip:axi_interconnect:2.1 axi_cpu_interconnect ]
  set_property -dict [list \
    CONFIG.NUM_MI {9} \
    CONFIG.NUM_SI {2} \
  ] $axi_cpu_interconnect


  # Create instance: axi_dmac_audio, and set properties
  set axi_dmac_audio [ create_bd_cell -type ip -vlnv analog.com:user:axi_dmac:1.0 axi_dmac_audio ]
  set_property -dict [list \
    CONFIG.DISABLE_DEBUG_REGISTERS {true} \
    CONFIG.DMA_DATA_WIDTH_DEST {64} \
    CONFIG.DMA_DATA_WIDTH_SRC {32} \
    CONFIG.DMA_TYPE_SRC {1} \
  ] $axi_dmac_audio


  # Create instance: axi_gpio_rx, and set properties
  set axi_gpio_rx [ create_bd_cell -type ip -vlnv xilinx.com:ip:axi_gpio:2.0 axi_gpio_rx ]
  set_property -dict [list \
    CONFIG.C_ALL_OUTPUTS {1} \
    CONFIG.C_ALL_OUTPUTS_2 {1} \
    CONFIG.C_GPIO2_WIDTH {32} \
    CONFIG.C_GPIO_WIDTH {32} \
    CONFIG.C_IS_DUAL {1} \
  ] $axi_gpio_rx


  # Create instance: axi_gpio_tx, and set properties
  set axi_gpio_tx [ create_bd_cell -type ip -vlnv xilinx.com:ip:axi_gpio:2.0 axi_gpio_tx ]
  set_property -dict [list \
    CONFIG.C_ALL_OUTPUTS {1} \
    CONFIG.C_ALL_OUTPUTS_2 {1} \
    CONFIG.C_IS_DUAL {1} \
  ] $axi_gpio_tx


  # Create instance: axi_iic_main, and set properties
  set axi_iic_main [ create_bd_cell -type ip -vlnv xilinx.com:ip:axi_iic:2.1 axi_iic_main ]

  # Create instance: axi_spi, and set properties
  set axi_spi [ create_bd_cell -type ip -vlnv xilinx.com:ip:axi_quad_spi:3.2 axi_spi ]
  set_property -dict [list \
    CONFIG.C_NUM_SS_BITS {1} \
    CONFIG.C_SCK_RATIO {8} \
    CONFIG.C_USE_STARTUP {0} \
  ] $axi_spi


  # Create instance: axi_tdd_0
  create_hier_cell_axi_tdd_0 [current_bd_instance .] axi_tdd_0

  # Create instance: burst_enabled, and set properties
  set burst_enabled [ create_bd_cell -type ip -vlnv xilinx.com:ip:xlslice:1.0 burst_enabled ]
  set_property -dict [list \
    CONFIG.DIN_FROM {2} \
    CONFIG.DIN_TO {2} \
    CONFIG.DIN_WIDTH {32} \
  ] $burst_enabled


  # Create instance: burst_gate, and set properties
  set block_name burst_gate
  set block_cell_name burst_gate
  if { [catch {set burst_gate [create_bd_cell -type module -reference $block_name $block_cell_name] } errmsg] } {
     catch {common::send_gid_msg -ssname BD::TCL -id 2095 -severity "ERROR" "Unable to add referenced block <$block_name>. Please add the files for ${block_name}'s definition into the project."}
     return 1
   } elseif { $burst_gate eq "" } {
     catch {common::send_gid_msg -ssname BD::TCL -id 2096 -severity "ERROR" "Unable to referenced block <$block_name>. Please add the files for ${block_name}'s definition into the project."}
     return 1
   }
  
  # Create instance: burst_trigger, and set properties
  set burst_trigger [ create_bd_cell -type ip -vlnv xilinx.com:ip:xlslice:1.0 burst_trigger ]
  set_property -dict [list \
    CONFIG.DIN_FROM {1} \
    CONFIG.DIN_TO {1} \
    CONFIG.DIN_WIDTH {32} \
  ] $burst_trigger


  # Create instance: cic_config, and set properties
  set cic_config [ create_bd_cell -type ip -vlnv xilinx.com:ip:xlslice:1.0 cic_config ]
  set_property -dict [list \
    CONFIG.DIN_FROM {11} \
    CONFIG.DIN_TO {4} \
  ] $cic_config


  # Create instance: cic_config_valid, and set properties
  set cic_config_valid [ create_bd_cell -type ip -vlnv xilinx.com:ip:xlslice:1.0 cic_config_valid ]
  set_property -dict [list \
    CONFIG.DIN_FROM {3} \
    CONFIG.DIN_TO {3} \
    CONFIG.DIN_WIDTH {32} \
    CONFIG.DOUT_WIDTH {1} \
  ] $cic_config_valid


  # Create instance: cpack, and set properties
  set cpack [ create_bd_cell -type ip -vlnv analog.com:user:util_cpack2:1.0 cpack ]

  # Create instance: dds_valid, and set properties
  set dds_valid [ create_bd_cell -type ip -vlnv xilinx.com:ip:xlslice:1.0 dds_valid ]
  set_property -dict [list \
    CONFIG.DIN_FROM {12} \
    CONFIG.DIN_TO {12} \
  ] $dds_valid


  # Create instance: dsp_mux_rx_0, and set properties
  set block_name dsp_mux_rx
  set block_cell_name dsp_mux_rx_0
  if { [catch {set dsp_mux_rx_0 [create_bd_cell -type module -reference $block_name $block_cell_name] } errmsg] } {
     catch {common::send_gid_msg -ssname BD::TCL -id 2095 -severity "ERROR" "Unable to add referenced block <$block_name>. Please add the files for ${block_name}'s definition into the project."}
     return 1
   } elseif { $dsp_mux_rx_0 eq "" } {
     catch {common::send_gid_msg -ssname BD::TCL -id 2096 -severity "ERROR" "Unable to referenced block <$block_name>. Please add the files for ${block_name}'s definition into the project."}
     return 1
   }
  
  # Create instance: dsp_mux_tx, and set properties
  set block_name dsp_mux_tx
  set block_cell_name dsp_mux_tx
  if { [catch {set dsp_mux_tx [create_bd_cell -type module -reference $block_name $block_cell_name] } errmsg] } {
     catch {common::send_gid_msg -ssname BD::TCL -id 2095 -severity "ERROR" "Unable to add referenced block <$block_name>. Please add the files for ${block_name}'s definition into the project."}
     return 1
   } elseif { $dsp_mux_tx eq "" } {
     catch {common::send_gid_msg -ssname BD::TCL -id 2096 -severity "ERROR" "Unable to referenced block <$block_name>. Please add the files for ${block_name}'s definition into the project."}
     return 1
   }
  
  # Create instance: logic_inv, and set properties
  set logic_inv [ create_bd_cell -type ip -vlnv xilinx.com:ip:util_vector_logic:2.0 logic_inv ]
  set_property -dict [list \
    CONFIG.C_OPERATION {not} \
    CONFIG.C_SIZE {1} \
  ] $logic_inv


  # Create instance: or_tx_reset, and set properties
  set or_tx_reset [ create_bd_cell -type ip -vlnv xilinx.com:ip:util_vector_logic:2.0 or_tx_reset ]
  set_property -dict [list \
    CONFIG.C_OPERATION {or} \
    CONFIG.C_SIZE {1} \
  ] $or_tx_reset


  # Create instance: reset, and set properties
  set reset [ create_bd_cell -type ip -vlnv xilinx.com:ip:xlslice:1.0 reset ]
  set_property -dict [list \
    CONFIG.DIN_FROM {0} \
    CONFIG.DIN_TO {0} \
    CONFIG.DIN_WIDTH {32} \
  ] $reset


  # Create instance: rst_axi_ad9361_100M, and set properties
  set rst_axi_ad9361_100M [ create_bd_cell -type ip -vlnv xilinx.com:ip:proc_sys_reset:5.0 rst_axi_ad9361_100M ]

  # Create instance: rx_antenna_ctrl, and set properties
  set rx_antenna_ctrl [ create_bd_cell -type ip -vlnv xilinx.com:ip:xlslice:1.0 rx_antenna_ctrl ]
  set_property -dict [list \
    CONFIG.DIN_FROM {13} \
    CONFIG.DIN_TO {13} \
  ] $rx_antenna_ctrl


  # Create instance: rx_cic_i, and set properties
  set rx_cic_i [ create_bd_cell -type ip -vlnv xilinx.com:ip:cic_compiler:4.0 rx_cic_i ]
  set_property -dict [list \
    CONFIG.Clock_Frequency {2.4} \
    CONFIG.Filter_Type {Decimation} \
    CONFIG.Fixed_Or_Initial_Rate {4} \
    CONFIG.HAS_ARESETN {true} \
    CONFIG.Input_Data_Width {32} \
    CONFIG.Input_Sample_Frequency {2.4} \
    CONFIG.Maximum_Rate {64} \
    CONFIG.Minimum_Rate {4} \
    CONFIG.Number_Of_Channels {1} \
    CONFIG.Number_Of_Stages {4} \
    CONFIG.Output_Data_Width {56} \
    CONFIG.Quantization {Full_Precision} \
    CONFIG.SamplePeriod {1} \
    CONFIG.Sample_Rate_Changes {Programmable} \
    CONFIG.Use_Xtreme_DSP_Slice {true} \
  ] $rx_cic_i


  # Create instance: rx_cic_q, and set properties
  set rx_cic_q [ create_bd_cell -type ip -vlnv xilinx.com:ip:cic_compiler:4.0 rx_cic_q ]
  set_property -dict [list \
    CONFIG.Clock_Frequency {2.4} \
    CONFIG.Filter_Type {Decimation} \
    CONFIG.Fixed_Or_Initial_Rate {4} \
    CONFIG.HAS_ARESETN {true} \
    CONFIG.Input_Data_Width {32} \
    CONFIG.Input_Sample_Frequency {2.4} \
    CONFIG.Maximum_Rate {64} \
    CONFIG.Minimum_Rate {4} \
    CONFIG.Number_Of_Channels {1} \
    CONFIG.Number_Of_Stages {4} \
    CONFIG.Output_Data_Width {56} \
    CONFIG.Quantization {Full_Precision} \
    CONFIG.SamplePeriod {1} \
    CONFIG.Sample_Rate_Changes {Programmable} \
    CONFIG.Use_Xtreme_DSP_Slice {true} \
  ] $rx_cic_q


  # Create instance: rx_cmpy, and set properties
  set rx_cmpy [ create_bd_cell -type ip -vlnv xilinx.com:ip:cmpy:6.0 rx_cmpy ]
  set_property -dict [list \
    CONFIG.ARESETN {true} \
    CONFIG.MinimumLatency {4} \
    CONFIG.OptimizeGoal {Performance} \
    CONFIG.OutputWidth {32} \
  ] $rx_cmpy


  # Create instance: rx_cmpy_i, and set properties
  set rx_cmpy_i [ create_bd_cell -type ip -vlnv xilinx.com:ip:xlslice:1.0 rx_cmpy_i ]
  set_property -dict [list \
    CONFIG.DIN_FROM {31} \
    CONFIG.DIN_WIDTH {64} \
  ] $rx_cmpy_i


  # Create instance: rx_cmpy_q, and set properties
  set rx_cmpy_q [ create_bd_cell -type ip -vlnv xilinx.com:ip:xlslice:1.0 rx_cmpy_q ]
  set_property -dict [list \
    CONFIG.DIN_FROM {63} \
    CONFIG.DIN_TO {32} \
    CONFIG.DIN_WIDTH {64} \
  ] $rx_cmpy_q


  # Create instance: rx_data_fifo, and set properties
  set rx_data_fifo [ create_bd_cell -type ip -vlnv xilinx.com:ip:axis_data_fifo:2.0 rx_data_fifo ]
  set_property -dict [list \
    CONFIG.FIFO_DEPTH {256} \
    CONFIG.FIFO_MEMORY_TYPE {auto} \
  ] $rx_data_fifo


  # Create instance: rx_dds_compiler, and set properties
  set rx_dds_compiler [ create_bd_cell -type ip -vlnv xilinx.com:ip:dds_compiler:6.0 rx_dds_compiler ]
  set_property -dict [list \
    CONFIG.Channels {1} \
    CONFIG.DATA_Has_TLAST {Not_Required} \
    CONFIG.DDS_Clock_Rate {61.44} \
    CONFIG.DSP48_Use {Maximal} \
    CONFIG.Frequency_Resolution {0.014} \
    CONFIG.Has_ARESETn {true} \
    CONFIG.Has_Phase_Out {false} \
    CONFIG.Latency {8} \
    CONFIG.M_DATA_Has_TUSER {Not_Required} \
    CONFIG.M_PHASE_Has_TUSER {Not_Required} \
    CONFIG.Memory_Type {Block_ROM} \
    CONFIG.Noise_Shaping {None} \
    CONFIG.Optimization_Goal {Area} \
    CONFIG.Output_Frequency1 {0} \
    CONFIG.Output_Width {16} \
    CONFIG.PINC1 {0} \
    CONFIG.Parameter_Entry {Hardware_Parameters} \
    CONFIG.Phase_Increment {Programmable} \
    CONFIG.Phase_Width {32} \
    CONFIG.S_PHASE_Has_TUSER {Not_Required} \
    CONFIG.Spurious_Free_Dynamic_Range {96} \
  ] $rx_dds_compiler


  # Create instance: rx_fir, and set properties
  set rx_fir [ create_bd_cell -type ip -vlnv xilinx.com:ip:fir_compiler:7.2 rx_fir ]
  set_property -dict [list \
    CONFIG.Channel_Sequence {Basic} \
    CONFIG.Clock_Frequency {300.0} \
    CONFIG.Coefficient_Buffer_Type {Block} \
    CONFIG.Coefficient_Fractional_Bits {0} \
    CONFIG.Coefficient_Structure {Inferred} \
    CONFIG.Coefficient_Width {16} \
    CONFIG.ColumnConfig {1} \
    CONFIG.DATA_Has_TLAST {Not_Required} \
    CONFIG.Data_Buffer_Type {Block} \
    CONFIG.Decimation_Rate {4} \
    CONFIG.Filter_Architecture {Systolic_Multiply_Accumulate} \
    CONFIG.Filter_Type {Decimation} \
    CONFIG.Has_ARESETn {true} \
    CONFIG.Interpolation_Rate {1} \
    CONFIG.M_DATA_Has_TUSER {Not_Required} \
    CONFIG.Number_Channels {1} \
    CONFIG.Number_Paths {2} \
    CONFIG.Output_Rounding_Mode {Truncate_LSBs} \
    CONFIG.Output_Width {16} \
    CONFIG.Quantization {Integer_Coefficients} \
    CONFIG.RateSpecification {Input_Sample_Period} \
    CONFIG.S_DATA_Has_TUSER {Not_Required} \
    CONFIG.SamplePeriod {61} \
    CONFIG.Sample_Frequency {0.001} \
    CONFIG.Select_Pattern {All} \
    CONFIG.Zero_Pack_Factor {1} \
  ] $rx_fir


  # Create instance: rx_iq_packer, and set properties
  set block_name iq_packer
  set block_cell_name rx_iq_packer
  if { [catch {set rx_iq_packer [create_bd_cell -type module -reference $block_name $block_cell_name] } errmsg] } {
     catch {common::send_gid_msg -ssname BD::TCL -id 2095 -severity "ERROR" "Unable to add referenced block <$block_name>. Please add the files for ${block_name}'s definition into the project."}
     return 1
   } elseif { $rx_iq_packer eq "" } {
     catch {common::send_gid_msg -ssname BD::TCL -id 2096 -severity "ERROR" "Unable to referenced block <$block_name>. Please add the files for ${block_name}'s definition into the project."}
     return 1
   }
  
  # Create instance: rx_raw_data, and set properties
  set rx_raw_data [ create_bd_cell -type ip -vlnv xilinx.com:ip:xlconcat:2.1 rx_raw_data ]
  set_property -dict [list \
    CONFIG.IN0_WIDTH {16} \
    CONFIG.IN1_WIDTH {16} \
  ] $rx_raw_data


  # Create instance: sys_concat_intc, and set properties
  set sys_concat_intc [ create_bd_cell -type ip -vlnv xilinx.com:ip:xlconcat:2.1 sys_concat_intc ]
  set_property CONFIG.NUM_PORTS {16} $sys_concat_intc


  # Create instance: sys_ps7, and set properties
  set sys_ps7 [ create_bd_cell -type ip -vlnv xilinx.com:ip:processing_system7:5.5 sys_ps7 ]
  set_property -dict [list \
    CONFIG.PCW_ACT_APU_PERIPHERAL_FREQMHZ {666.666687} \
    CONFIG.PCW_ACT_CAN_PERIPHERAL_FREQMHZ {10.000000} \
    CONFIG.PCW_ACT_DCI_PERIPHERAL_FREQMHZ {10.158730} \
    CONFIG.PCW_ACT_ENET0_PERIPHERAL_FREQMHZ {125.000000} \
    CONFIG.PCW_ACT_ENET1_PERIPHERAL_FREQMHZ {10.000000} \
    CONFIG.PCW_ACT_FPGA0_PERIPHERAL_FREQMHZ {100.000000} \
    CONFIG.PCW_ACT_FPGA1_PERIPHERAL_FREQMHZ {200.000000} \
    CONFIG.PCW_ACT_FPGA2_PERIPHERAL_FREQMHZ {10.000000} \
    CONFIG.PCW_ACT_FPGA3_PERIPHERAL_FREQMHZ {10.000000} \
    CONFIG.PCW_ACT_PCAP_PERIPHERAL_FREQMHZ {200.000000} \
    CONFIG.PCW_ACT_QSPI_PERIPHERAL_FREQMHZ {200.000000} \
    CONFIG.PCW_ACT_SDIO_PERIPHERAL_FREQMHZ {100.000000} \
    CONFIG.PCW_ACT_SMC_PERIPHERAL_FREQMHZ {10.000000} \
    CONFIG.PCW_ACT_SPI_PERIPHERAL_FREQMHZ {166.666672} \
    CONFIG.PCW_ACT_TPIU_PERIPHERAL_FREQMHZ {200.000000} \
    CONFIG.PCW_ACT_TTC0_CLK0_PERIPHERAL_FREQMHZ {111.111115} \
    CONFIG.PCW_ACT_TTC0_CLK1_PERIPHERAL_FREQMHZ {111.111115} \
    CONFIG.PCW_ACT_TTC0_CLK2_PERIPHERAL_FREQMHZ {111.111115} \
    CONFIG.PCW_ACT_TTC1_CLK0_PERIPHERAL_FREQMHZ {111.111115} \
    CONFIG.PCW_ACT_TTC1_CLK1_PERIPHERAL_FREQMHZ {111.111115} \
    CONFIG.PCW_ACT_TTC1_CLK2_PERIPHERAL_FREQMHZ {111.111115} \
    CONFIG.PCW_ACT_UART_PERIPHERAL_FREQMHZ {100.000000} \
    CONFIG.PCW_ACT_WDT_PERIPHERAL_FREQMHZ {111.111115} \
    CONFIG.PCW_CLK0_FREQ {100000000} \
    CONFIG.PCW_CLK1_FREQ {200000000} \
    CONFIG.PCW_CLK2_FREQ {10000000} \
    CONFIG.PCW_CLK3_FREQ {10000000} \
    CONFIG.PCW_DDR_RAM_HIGHADDR {0x1FFFFFFF} \
    CONFIG.PCW_DM_WIDTH {4} \
    CONFIG.PCW_DQS_WIDTH {4} \
    CONFIG.PCW_DQ_WIDTH {32} \
    CONFIG.PCW_ENET0_ENET0_IO {MIO 16 .. 27} \
    CONFIG.PCW_ENET0_GRP_MDIO_ENABLE {1} \
    CONFIG.PCW_ENET0_GRP_MDIO_IO {MIO 52 .. 53} \
    CONFIG.PCW_ENET0_PERIPHERAL_CLKSRC {IO PLL} \
    CONFIG.PCW_ENET0_PERIPHERAL_ENABLE {1} \
    CONFIG.PCW_ENET0_PERIPHERAL_FREQMHZ {1000 Mbps} \
    CONFIG.PCW_ENET0_RESET_ENABLE {0} \
    CONFIG.PCW_ENET_RESET_ENABLE {1} \
    CONFIG.PCW_ENET_RESET_SELECT {Share reset pin} \
    CONFIG.PCW_EN_CLK1_PORT {1} \
    CONFIG.PCW_EN_EMIO_CD_SDIO0 {0} \
    CONFIG.PCW_EN_EMIO_ENET0 {0} \
    CONFIG.PCW_EN_EMIO_GPIO {1} \
    CONFIG.PCW_EN_EMIO_SPI0 {1} \
    CONFIG.PCW_EN_ENET0 {1} \
    CONFIG.PCW_EN_GPIO {1} \
    CONFIG.PCW_EN_QSPI {1} \
    CONFIG.PCW_EN_RST1_PORT {1} \
    CONFIG.PCW_EN_SDIO0 {1} \
    CONFIG.PCW_EN_SPI0 {1} \
    CONFIG.PCW_EN_UART1 {1} \
    CONFIG.PCW_EN_USB0 {1} \
    CONFIG.PCW_FCLK_CLK1_BUF {TRUE} \
    CONFIG.PCW_FPGA0_PERIPHERAL_FREQMHZ {100.0} \
    CONFIG.PCW_FPGA1_PERIPHERAL_FREQMHZ {200.0} \
    CONFIG.PCW_FPGA_FCLK0_ENABLE {1} \
    CONFIG.PCW_FPGA_FCLK1_ENABLE {1} \
    CONFIG.PCW_GPIO_EMIO_GPIO_ENABLE {1} \
    CONFIG.PCW_GPIO_EMIO_GPIO_IO {18} \
    CONFIG.PCW_GPIO_EMIO_GPIO_WIDTH {18} \
    CONFIG.PCW_GPIO_MIO_GPIO_ENABLE {1} \
    CONFIG.PCW_GPIO_MIO_GPIO_IO {MIO} \
    CONFIG.PCW_I2C0_PERIPHERAL_ENABLE {0} \
    CONFIG.PCW_I2C1_PERIPHERAL_ENABLE {0} \
    CONFIG.PCW_I2C_RESET_ENABLE {1} \
    CONFIG.PCW_IRQ_F2P_INTR {1} \
    CONFIG.PCW_IRQ_F2P_MODE {REVERSE} \
    CONFIG.PCW_MIO_0_IOTYPE {LVCMOS 1.8V} \
    CONFIG.PCW_MIO_0_PULLUP {enabled} \
    CONFIG.PCW_MIO_0_SLEW {slow} \
    CONFIG.PCW_MIO_10_IOTYPE {LVCMOS 1.8V} \
    CONFIG.PCW_MIO_10_PULLUP {enabled} \
    CONFIG.PCW_MIO_10_SLEW {slow} \
    CONFIG.PCW_MIO_11_IOTYPE {LVCMOS 1.8V} \
    CONFIG.PCW_MIO_11_PULLUP {enabled} \
    CONFIG.PCW_MIO_11_SLEW {slow} \
    CONFIG.PCW_MIO_12_IOTYPE {LVCMOS 1.8V} \
    CONFIG.PCW_MIO_12_PULLUP {enabled} \
    CONFIG.PCW_MIO_12_SLEW {slow} \
    CONFIG.PCW_MIO_13_IOTYPE {LVCMOS 1.8V} \
    CONFIG.PCW_MIO_13_PULLUP {enabled} \
    CONFIG.PCW_MIO_13_SLEW {slow} \
    CONFIG.PCW_MIO_14_IOTYPE {LVCMOS 1.8V} \
    CONFIG.PCW_MIO_14_PULLUP {enabled} \
    CONFIG.PCW_MIO_14_SLEW {slow} \
    CONFIG.PCW_MIO_15_IOTYPE {LVCMOS 1.8V} \
    CONFIG.PCW_MIO_15_PULLUP {enabled} \
    CONFIG.PCW_MIO_15_SLEW {slow} \
    CONFIG.PCW_MIO_16_IOTYPE {LVCMOS 1.8V} \
    CONFIG.PCW_MIO_16_PULLUP {enabled} \
    CONFIG.PCW_MIO_16_SLEW {slow} \
    CONFIG.PCW_MIO_17_IOTYPE {LVCMOS 1.8V} \
    CONFIG.PCW_MIO_17_PULLUP {enabled} \
    CONFIG.PCW_MIO_17_SLEW {slow} \
    CONFIG.PCW_MIO_18_IOTYPE {LVCMOS 1.8V} \
    CONFIG.PCW_MIO_18_PULLUP {enabled} \
    CONFIG.PCW_MIO_18_SLEW {slow} \
    CONFIG.PCW_MIO_19_IOTYPE {LVCMOS 1.8V} \
    CONFIG.PCW_MIO_19_PULLUP {enabled} \
    CONFIG.PCW_MIO_19_SLEW {slow} \
    CONFIG.PCW_MIO_1_IOTYPE {LVCMOS 1.8V} \
    CONFIG.PCW_MIO_1_PULLUP {enabled} \
    CONFIG.PCW_MIO_1_SLEW {slow} \
    CONFIG.PCW_MIO_20_IOTYPE {LVCMOS 1.8V} \
    CONFIG.PCW_MIO_20_PULLUP {enabled} \
    CONFIG.PCW_MIO_20_SLEW {slow} \
    CONFIG.PCW_MIO_21_IOTYPE {LVCMOS 1.8V} \
    CONFIG.PCW_MIO_21_PULLUP {enabled} \
    CONFIG.PCW_MIO_21_SLEW {slow} \
    CONFIG.PCW_MIO_22_IOTYPE {LVCMOS 1.8V} \
    CONFIG.PCW_MIO_22_PULLUP {enabled} \
    CONFIG.PCW_MIO_22_SLEW {slow} \
    CONFIG.PCW_MIO_23_IOTYPE {LVCMOS 1.8V} \
    CONFIG.PCW_MIO_23_PULLUP {enabled} \
    CONFIG.PCW_MIO_23_SLEW {slow} \
    CONFIG.PCW_MIO_24_IOTYPE {LVCMOS 1.8V} \
    CONFIG.PCW_MIO_24_PULLUP {enabled} \
    CONFIG.PCW_MIO_24_SLEW {slow} \
    CONFIG.PCW_MIO_25_IOTYPE {LVCMOS 1.8V} \
    CONFIG.PCW_MIO_25_PULLUP {enabled} \
    CONFIG.PCW_MIO_25_SLEW {slow} \
    CONFIG.PCW_MIO_26_IOTYPE {LVCMOS 1.8V} \
    CONFIG.PCW_MIO_26_PULLUP {enabled} \
    CONFIG.PCW_MIO_26_SLEW {slow} \
    CONFIG.PCW_MIO_27_IOTYPE {LVCMOS 1.8V} \
    CONFIG.PCW_MIO_27_PULLUP {enabled} \
    CONFIG.PCW_MIO_27_SLEW {slow} \
    CONFIG.PCW_MIO_28_IOTYPE {LVCMOS 1.8V} \
    CONFIG.PCW_MIO_28_PULLUP {enabled} \
    CONFIG.PCW_MIO_28_SLEW {slow} \
    CONFIG.PCW_MIO_29_IOTYPE {LVCMOS 1.8V} \
    CONFIG.PCW_MIO_29_PULLUP {enabled} \
    CONFIG.PCW_MIO_29_SLEW {slow} \
    CONFIG.PCW_MIO_2_IOTYPE {LVCMOS 1.8V} \
    CONFIG.PCW_MIO_2_SLEW {slow} \
    CONFIG.PCW_MIO_30_IOTYPE {LVCMOS 1.8V} \
    CONFIG.PCW_MIO_30_PULLUP {enabled} \
    CONFIG.PCW_MIO_30_SLEW {slow} \
    CONFIG.PCW_MIO_31_IOTYPE {LVCMOS 1.8V} \
    CONFIG.PCW_MIO_31_PULLUP {enabled} \
    CONFIG.PCW_MIO_31_SLEW {slow} \
    CONFIG.PCW_MIO_32_IOTYPE {LVCMOS 1.8V} \
    CONFIG.PCW_MIO_32_PULLUP {enabled} \
    CONFIG.PCW_MIO_32_SLEW {slow} \
    CONFIG.PCW_MIO_33_IOTYPE {LVCMOS 1.8V} \
    CONFIG.PCW_MIO_33_PULLUP {enabled} \
    CONFIG.PCW_MIO_33_SLEW {slow} \
    CONFIG.PCW_MIO_34_IOTYPE {LVCMOS 1.8V} \
    CONFIG.PCW_MIO_34_PULLUP {enabled} \
    CONFIG.PCW_MIO_34_SLEW {slow} \
    CONFIG.PCW_MIO_35_IOTYPE {LVCMOS 1.8V} \
    CONFIG.PCW_MIO_35_PULLUP {enabled} \
    CONFIG.PCW_MIO_35_SLEW {slow} \
    CONFIG.PCW_MIO_36_IOTYPE {LVCMOS 1.8V} \
    CONFIG.PCW_MIO_36_PULLUP {enabled} \
    CONFIG.PCW_MIO_36_SLEW {slow} \
    CONFIG.PCW_MIO_37_IOTYPE {LVCMOS 1.8V} \
    CONFIG.PCW_MIO_37_PULLUP {enabled} \
    CONFIG.PCW_MIO_37_SLEW {slow} \
    CONFIG.PCW_MIO_38_IOTYPE {LVCMOS 1.8V} \
    CONFIG.PCW_MIO_38_PULLUP {enabled} \
    CONFIG.PCW_MIO_38_SLEW {slow} \
    CONFIG.PCW_MIO_39_IOTYPE {LVCMOS 1.8V} \
    CONFIG.PCW_MIO_39_PULLUP {enabled} \
    CONFIG.PCW_MIO_39_SLEW {slow} \
    CONFIG.PCW_MIO_3_IOTYPE {LVCMOS 1.8V} \
    CONFIG.PCW_MIO_3_SLEW {slow} \
    CONFIG.PCW_MIO_40_IOTYPE {LVCMOS 1.8V} \
    CONFIG.PCW_MIO_40_PULLUP {enabled} \
    CONFIG.PCW_MIO_40_SLEW {slow} \
    CONFIG.PCW_MIO_41_IOTYPE {LVCMOS 1.8V} \
    CONFIG.PCW_MIO_41_PULLUP {enabled} \
    CONFIG.PCW_MIO_41_SLEW {slow} \
    CONFIG.PCW_MIO_42_IOTYPE {LVCMOS 1.8V} \
    CONFIG.PCW_MIO_42_PULLUP {enabled} \
    CONFIG.PCW_MIO_42_SLEW {slow} \
    CONFIG.PCW_MIO_43_IOTYPE {LVCMOS 1.8V} \
    CONFIG.PCW_MIO_43_PULLUP {enabled} \
    CONFIG.PCW_MIO_43_SLEW {slow} \
    CONFIG.PCW_MIO_44_IOTYPE {LVCMOS 1.8V} \
    CONFIG.PCW_MIO_44_PULLUP {enabled} \
    CONFIG.PCW_MIO_44_SLEW {slow} \
    CONFIG.PCW_MIO_45_IOTYPE {LVCMOS 1.8V} \
    CONFIG.PCW_MIO_45_PULLUP {enabled} \
    CONFIG.PCW_MIO_45_SLEW {slow} \
    CONFIG.PCW_MIO_46_IOTYPE {LVCMOS 1.8V} \
    CONFIG.PCW_MIO_46_PULLUP {enabled} \
    CONFIG.PCW_MIO_46_SLEW {slow} \
    CONFIG.PCW_MIO_47_IOTYPE {LVCMOS 1.8V} \
    CONFIG.PCW_MIO_47_PULLUP {enabled} \
    CONFIG.PCW_MIO_47_SLEW {slow} \
    CONFIG.PCW_MIO_48_IOTYPE {LVCMOS 1.8V} \
    CONFIG.PCW_MIO_48_PULLUP {enabled} \
    CONFIG.PCW_MIO_48_SLEW {slow} \
    CONFIG.PCW_MIO_49_IOTYPE {LVCMOS 1.8V} \
    CONFIG.PCW_MIO_49_PULLUP {disabled} \
    CONFIG.PCW_MIO_49_SLEW {slow} \
    CONFIG.PCW_MIO_4_IOTYPE {LVCMOS 1.8V} \
    CONFIG.PCW_MIO_4_SLEW {slow} \
    CONFIG.PCW_MIO_50_IOTYPE {LVCMOS 1.8V} \
    CONFIG.PCW_MIO_50_PULLUP {enabled} \
    CONFIG.PCW_MIO_50_SLEW {slow} \
    CONFIG.PCW_MIO_51_IOTYPE {LVCMOS 1.8V} \
    CONFIG.PCW_MIO_51_PULLUP {enabled} \
    CONFIG.PCW_MIO_51_SLEW {slow} \
    CONFIG.PCW_MIO_52_IOTYPE {LVCMOS 1.8V} \
    CONFIG.PCW_MIO_52_PULLUP {enabled} \
    CONFIG.PCW_MIO_52_SLEW {slow} \
    CONFIG.PCW_MIO_53_IOTYPE {LVCMOS 1.8V} \
    CONFIG.PCW_MIO_53_PULLUP {enabled} \
    CONFIG.PCW_MIO_53_SLEW {slow} \
    CONFIG.PCW_MIO_5_IOTYPE {LVCMOS 1.8V} \
    CONFIG.PCW_MIO_5_SLEW {slow} \
    CONFIG.PCW_MIO_6_IOTYPE {LVCMOS 1.8V} \
    CONFIG.PCW_MIO_6_SLEW {slow} \
    CONFIG.PCW_MIO_7_IOTYPE {LVCMOS 1.8V} \
    CONFIG.PCW_MIO_7_SLEW {slow} \
    CONFIG.PCW_MIO_8_IOTYPE {LVCMOS 1.8V} \
    CONFIG.PCW_MIO_8_SLEW {slow} \
    CONFIG.PCW_MIO_9_IOTYPE {LVCMOS 1.8V} \
    CONFIG.PCW_MIO_9_PULLUP {enabled} \
    CONFIG.PCW_MIO_9_SLEW {slow} \
    CONFIG.PCW_MIO_PRIMITIVE {54} \
    CONFIG.PCW_MIO_TREE_PERIPHERALS {GPIO#Quad SPI Flash#Quad SPI Flash#Quad SPI Flash#Quad SPI Flash#Quad SPI Flash#Quad SPI Flash#GPIO#GPIO#GPIO#GPIO#GPIO#UART 1#UART 1#GPIO#GPIO#Enet 0#Enet 0#Enet 0#Enet\
0#Enet 0#Enet 0#Enet 0#Enet 0#Enet 0#Enet 0#Enet 0#Enet 0#USB 0#USB 0#USB 0#USB 0#USB 0#USB 0#USB 0#USB 0#USB 0#USB 0#USB 0#USB 0#SD 0#SD 0#SD 0#SD 0#SD 0#SD 0#USB Reset#SD 0#GPIO#GPIO#GPIO#GPIO#Enet 0#Enet\
0} \
    CONFIG.PCW_MIO_TREE_SIGNALS {gpio[0]#qspi0_ss_b#qspi0_io[0]#qspi0_io[1]#qspi0_io[2]#qspi0_io[3]/HOLD_B#qspi0_sclk#gpio[7]#gpio[8]#gpio[9]#gpio[10]#gpio[11]#tx#rx#gpio[14]#gpio[15]#tx_clk#txd[0]#txd[1]#txd[2]#txd[3]#tx_ctl#rx_clk#rxd[0]#rxd[1]#rxd[2]#rxd[3]#rx_ctl#data[4]#dir#stp#nxt#data[0]#data[1]#data[2]#data[3]#clk#data[5]#data[6]#data[7]#clk#cmd#data[0]#data[1]#data[2]#data[3]#reset#cd#gpio[48]#gpio[49]#gpio[50]#gpio[51]#mdc#mdio}\
\
    CONFIG.PCW_PACKAGE_NAME {clg400} \
    CONFIG.PCW_PRESET_BANK0_VOLTAGE {LVCMOS 1.8V} \
    CONFIG.PCW_PRESET_BANK1_VOLTAGE {LVCMOS 1.8V} \
    CONFIG.PCW_QSPI_GRP_FBCLK_ENABLE {0} \
    CONFIG.PCW_QSPI_GRP_IO1_ENABLE {0} \
    CONFIG.PCW_QSPI_GRP_SINGLE_SS_ENABLE {1} \
    CONFIG.PCW_QSPI_GRP_SINGLE_SS_IO {MIO 1 .. 6} \
    CONFIG.PCW_QSPI_GRP_SS1_ENABLE {0} \
    CONFIG.PCW_QSPI_PERIPHERAL_ENABLE {1} \
    CONFIG.PCW_QSPI_PERIPHERAL_FREQMHZ {200} \
    CONFIG.PCW_QSPI_QSPI_IO {MIO 1 .. 6} \
    CONFIG.PCW_SD0_GRP_CD_ENABLE {1} \
    CONFIG.PCW_SD0_GRP_CD_IO {MIO 47} \
    CONFIG.PCW_SD0_GRP_POW_ENABLE {0} \
    CONFIG.PCW_SD0_GRP_WP_ENABLE {0} \
    CONFIG.PCW_SD0_PERIPHERAL_ENABLE {1} \
    CONFIG.PCW_SD0_SD0_IO {MIO 40 .. 45} \
    CONFIG.PCW_SDIO_PERIPHERAL_FREQMHZ {100} \
    CONFIG.PCW_SDIO_PERIPHERAL_VALID {1} \
    CONFIG.PCW_SINGLE_QSPI_DATA_MODE {x4} \
    CONFIG.PCW_SPI0_PERIPHERAL_ENABLE {1} \
    CONFIG.PCW_SPI0_SPI0_IO {EMIO} \
    CONFIG.PCW_SPI1_PERIPHERAL_ENABLE {0} \
    CONFIG.PCW_SPI_PERIPHERAL_FREQMHZ {166.666666} \
    CONFIG.PCW_SPI_PERIPHERAL_VALID {1} \
    CONFIG.PCW_TTC0_PERIPHERAL_ENABLE {0} \
    CONFIG.PCW_UART1_GRP_FULL_ENABLE {0} \
    CONFIG.PCW_UART1_PERIPHERAL_ENABLE {1} \
    CONFIG.PCW_UART1_UART1_IO {MIO 12 .. 13} \
    CONFIG.PCW_UART_PERIPHERAL_FREQMHZ {100} \
    CONFIG.PCW_UART_PERIPHERAL_VALID {1} \
    CONFIG.PCW_UIPARAM_ACT_DDR_FREQ_MHZ {533.333374} \
    CONFIG.PCW_UIPARAM_DDR_BOARD_DELAY0 {0.241} \
    CONFIG.PCW_UIPARAM_DDR_BOARD_DELAY1 {0.240} \
    CONFIG.PCW_UIPARAM_DDR_BUS_WIDTH {16 Bit} \
    CONFIG.PCW_UIPARAM_DDR_DQS_TO_CLK_DELAY_0 {0.048} \
    CONFIG.PCW_UIPARAM_DDR_DQS_TO_CLK_DELAY_1 {0.050} \
    CONFIG.PCW_UIPARAM_DDR_ECC {Disabled} \
    CONFIG.PCW_UIPARAM_DDR_PARTNO {MT41K256M16 RE-125} \
    CONFIG.PCW_UIPARAM_DDR_TRAIN_DATA_EYE {1} \
    CONFIG.PCW_UIPARAM_DDR_TRAIN_READ_GATE {1} \
    CONFIG.PCW_UIPARAM_DDR_TRAIN_WRITE_LEVEL {1} \
    CONFIG.PCW_UIPARAM_DDR_USE_INTERNAL_VREF {0} \
    CONFIG.PCW_USB0_PERIPHERAL_ENABLE {1} \
    CONFIG.PCW_USB0_RESET_ENABLE {1} \
    CONFIG.PCW_USB0_RESET_IO {MIO 46} \
    CONFIG.PCW_USB0_USB0_IO {MIO 28 .. 39} \
    CONFIG.PCW_USB_RESET_ENABLE {1} \
    CONFIG.PCW_USB_RESET_SELECT {Share reset pin} \
    CONFIG.PCW_USE_FABRIC_INTERRUPT {1} \
    CONFIG.PCW_USE_S_AXI_HP0 {1} \
    CONFIG.PCW_USE_S_AXI_HP1 {1} \
    CONFIG.PCW_USE_S_AXI_HP2 {1} \
  ] $sys_ps7


  # Create instance: sys_rstgen, and set properties
  set sys_rstgen [ create_bd_cell -type ip -vlnv xilinx.com:ip:proc_sys_reset:5.0 sys_rstgen ]
  set_property CONFIG.C_EXT_RST_WIDTH {1} $sys_rstgen


  # Create instance: tx_antenna_ctrl, and set properties
  set tx_antenna_ctrl [ create_bd_cell -type ip -vlnv xilinx.com:ip:xlslice:1.0 tx_antenna_ctrl ]
  set_property -dict [list \
    CONFIG.DIN_FROM {1} \
    CONFIG.DIN_TO {1} \
  ] $tx_antenna_ctrl


  # Create instance: tx_cic_config, and set properties
  set tx_cic_config [ create_bd_cell -type ip -vlnv xilinx.com:ip:xlslice:1.0 tx_cic_config ]
  set_property -dict [list \
    CONFIG.DIN_FROM {11} \
    CONFIG.DIN_TO {4} \
  ] $tx_cic_config


  # Create instance: tx_cic_config_valid, and set properties
  set tx_cic_config_valid [ create_bd_cell -type ip -vlnv xilinx.com:ip:xlslice:1.0 tx_cic_config_valid ]
  set_property -dict [list \
    CONFIG.DIN_FROM {3} \
    CONFIG.DIN_TO {3} \
  ] $tx_cic_config_valid


  # Create instance: tx_cic_data, and set properties
  set tx_cic_data [ create_bd_cell -type ip -vlnv xilinx.com:ip:xlconcat:2.1 tx_cic_data ]
  set_property -dict [list \
    CONFIG.IN0_WIDTH {16} \
    CONFIG.IN1_WIDTH {16} \
  ] $tx_cic_data


  # Create instance: tx_cic_i, and set properties
  set tx_cic_i [ create_bd_cell -type ip -vlnv xilinx.com:ip:cic_compiler:4.0 tx_cic_i ]
  set_property -dict [list \
    CONFIG.Clock_Frequency {400} \
    CONFIG.Differential_Delay {1} \
    CONFIG.HAS_DOUT_TREADY {true} \
    CONFIG.Input_Data_Width {16} \
    CONFIG.Input_Sample_Frequency {100} \
    CONFIG.Maximum_Rate {64} \
    CONFIG.Minimum_Rate {4} \
    CONFIG.Number_Of_Stages {3} \
    CONFIG.Output_Data_Width {16} \
    CONFIG.Quantization {Truncation} \
    CONFIG.Sample_Rate_Changes {Programmable} \
    CONFIG.Use_Xtreme_DSP_Slice {true} \
  ] $tx_cic_i


  # Create instance: tx_cic_q, and set properties
  set tx_cic_q [ create_bd_cell -type ip -vlnv xilinx.com:ip:cic_compiler:4.0 tx_cic_q ]
  set_property -dict [list \
    CONFIG.Clock_Frequency {400} \
    CONFIG.Differential_Delay {1} \
    CONFIG.HAS_DOUT_TREADY {true} \
    CONFIG.Input_Data_Width {16} \
    CONFIG.Input_Sample_Frequency {100} \
    CONFIG.Maximum_Rate {64} \
    CONFIG.Minimum_Rate {4} \
    CONFIG.Number_Of_Stages {3} \
    CONFIG.Output_Data_Width {16} \
    CONFIG.Quantization {Truncation} \
    CONFIG.Sample_Rate_Changes {Programmable} \
  ] $tx_cic_q


  # Create instance: tx_cmpy, and set properties
  set tx_cmpy [ create_bd_cell -type ip -vlnv xilinx.com:ip:cmpy:6.0 tx_cmpy ]
  set_property -dict [list \
    CONFIG.MinimumLatency {4} \
    CONFIG.OptimizeGoal {Performance} \
    CONFIG.OutputWidth {16} \
  ] $tx_cmpy


  # Create instance: tx_dds_compiler, and set properties
  set tx_dds_compiler [ create_bd_cell -type ip -vlnv xilinx.com:ip:dds_compiler:6.0 tx_dds_compiler ]
  set_property -dict [list \
    CONFIG.DATA_Has_TLAST {Not_Required} \
    CONFIG.DDS_Clock_Rate {61.44} \
    CONFIG.DSP48_Use {Maximal} \
    CONFIG.Frequency_Resolution {0.4} \
    CONFIG.Has_Phase_Out {false} \
    CONFIG.Latency {8} \
    CONFIG.M_DATA_Has_TUSER {Not_Required} \
    CONFIG.Noise_Shaping {None} \
    CONFIG.Output_Frequency1 {0} \
    CONFIG.Output_Width {16} \
    CONFIG.PINC1 {0} \
    CONFIG.Parameter_Entry {Hardware_Parameters} \
    CONFIG.Phase_Increment {Programmable} \
    CONFIG.Phase_Width {32} \
    CONFIG.S_PHASE_Has_TUSER {Not_Required} \
    CONFIG.Spurious_Free_Dynamic_Range {96} \
  ] $tx_dds_compiler


  # Create instance: tx_dds_valid, and set properties
  set tx_dds_valid [ create_bd_cell -type ip -vlnv xilinx.com:ip:xlslice:1.0 tx_dds_valid ]
  set_property -dict [list \
    CONFIG.DIN_FROM {2} \
    CONFIG.DIN_TO {2} \
  ] $tx_dds_valid


  # Create instance: tx_dsp_enable, and set properties
  set tx_dsp_enable [ create_bd_cell -type ip -vlnv xilinx.com:ip:xlslice:1.0 tx_dsp_enable ]

  # Create instance: tx_fir, and set properties
  set tx_fir [ create_bd_cell -type ip -vlnv xilinx.com:ip:fir_compiler:7.2 tx_fir ]
  set_property -dict [list \
    CONFIG.Clock_Frequency {61.44} \
    CONFIG.CoefficientSource {Vector} \
    CONFIG.CoefficientVector {44,194,510,1046,1748,2464,2925,2853,2076,678,-959,-2235,-2562,-1653,253,2382,3671,3241,881,-2670,-5835,-6725,-3909,2890,12478,22452,29972,32767,29972,22452,12478,2890,-3909,-6725,-5835,-2670,881,3241,3671,2382,253,-1653,-2562,-2235,-959,678,2076,2853,2925,2464,1748,1046,510,194,44}\
\
    CONFIG.Coefficient_Buffer_Type {Block} \
    CONFIG.Coefficient_Fractional_Bits {0} \
    CONFIG.Coefficient_Sets {1} \
    CONFIG.Coefficient_Sign {Signed} \
    CONFIG.Coefficient_Structure {Inferred} \
    CONFIG.Coefficient_Width {16} \
    CONFIG.ColumnConfig {1} \
    CONFIG.DATA_Has_TLAST {Not_Required} \
    CONFIG.Data_Buffer_Type {Block} \
    CONFIG.Data_Fractional_Bits {0} \
    CONFIG.Data_Width {16} \
    CONFIG.Filter_Architecture {Systolic_Multiply_Accumulate} \
    CONFIG.Filter_Type {Interpolation} \
    CONFIG.Interpolation_Rate {4} \
    CONFIG.M_DATA_Has_TREADY {true} \
    CONFIG.M_DATA_Has_TUSER {Not_Required} \
    CONFIG.Number_Channels {1} \
    CONFIG.Number_Paths {2} \
    CONFIG.Output_Rounding_Mode {Symmetric_Rounding_to_Zero} \
    CONFIG.Output_Width {16} \
    CONFIG.Quantization {Integer_Coefficients} \
    CONFIG.RateSpecification {Frequency_Specification} \
    CONFIG.S_DATA_Has_TUSER {Not_Required} \
    CONFIG.SamplePeriod {1} \
    CONFIG.Sample_Frequency {1.92} \
    CONFIG.Zero_Pack_Factor {1} \
  ] $tx_fir


  # Create instance: tx_fir_i, and set properties
  set tx_fir_i [ create_bd_cell -type ip -vlnv xilinx.com:ip:xlslice:1.0 tx_fir_i ]
  set_property -dict [list \
    CONFIG.DIN_FROM {15} \
    CONFIG.DIN_TO {0} \
    CONFIG.DIN_WIDTH {32} \
    CONFIG.DOUT_WIDTH {16} \
  ] $tx_fir_i


  # Create instance: tx_fir_q, and set properties
  set tx_fir_q [ create_bd_cell -type ip -vlnv xilinx.com:ip:xlslice:1.0 tx_fir_q ]
  set_property -dict [list \
    CONFIG.DIN_FROM {31} \
    CONFIG.DIN_TO {16} \
    CONFIG.DIN_WIDTH {32} \
    CONFIG.DOUT_WIDTH {16} \
  ] $tx_fir_q


  # Create instance: tx_i, and set properties
  set tx_i [ create_bd_cell -type ip -vlnv xilinx.com:ip:xlslice:1.0 tx_i ]
  set_property CONFIG.DIN_FROM {15} $tx_i


  # Create instance: tx_q, and set properties
  set tx_q [ create_bd_cell -type ip -vlnv xilinx.com:ip:xlslice:1.0 tx_q ]
  set_property -dict [list \
    CONFIG.DIN_FROM {31} \
    CONFIG.DIN_TO {16} \
  ] $tx_q


  # Create instance: tx_raw_data, and set properties
  set tx_raw_data [ create_bd_cell -type ip -vlnv xilinx.com:ip:xlconcat:2.1 tx_raw_data ]
  set_property -dict [list \
    CONFIG.IN0_WIDTH {16} \
    CONFIG.IN1_WIDTH {16} \
  ] $tx_raw_data


  # Create instance: tx_strobe, and set properties
  set tx_strobe [ create_bd_cell -type ip -vlnv xilinx.com:ip:xlslice:1.0 tx_strobe ]
  set_property -dict [list \
    CONFIG.DIN_FROM {27} \
    CONFIG.DIN_TO {12} \
  ] $tx_strobe


  # Create instance: tx_strobe_gen, and set properties
  set block_name tx_strobe_gen
  set block_cell_name tx_strobe_gen
  if { [catch {set tx_strobe_gen [create_bd_cell -type module -reference $block_name $block_cell_name] } errmsg] } {
     catch {common::send_gid_msg -ssname BD::TCL -id 2095 -severity "ERROR" "Unable to add referenced block <$block_name>. Please add the files for ${block_name}'s definition into the project."}
     return 1
   } elseif { $tx_strobe_gen eq "" } {
     catch {common::send_gid_msg -ssname BD::TCL -id 2096 -severity "ERROR" "Unable to referenced block <$block_name>. Please add the files for ${block_name}'s definition into the project."}
     return 1
   }
  
  # Create instance: tx_upack, and set properties
  set tx_upack [ create_bd_cell -type ip -vlnv analog.com:user:util_upack2:1.0 tx_upack ]

  # Create interface connections
  connect_bd_intf_net -intf_net S00_AXI_1 [get_bd_intf_pins axi_cpu_interconnect/S00_AXI] [get_bd_intf_pins sys_ps7/M_AXI_GP0]
  connect_bd_intf_net -intf_net axi_ad9361_adc_dma_m_dest_axi [get_bd_intf_pins axi_ad9361_adc_dma/m_dest_axi] [get_bd_intf_pins sys_ps7/S_AXI_HP1]
  connect_bd_intf_net -intf_net axi_ad9361_dac_dma_m_axis [get_bd_intf_pins axi_ad9361_dac_dma/m_axis] [get_bd_intf_pins tx_upack/s_axis]
  connect_bd_intf_net -intf_net axi_ad9361_dac_dma_m_src_axi [get_bd_intf_pins axi_ad9361_dac_dma/m_src_axi] [get_bd_intf_pins sys_ps7/S_AXI_HP2]
  connect_bd_intf_net -intf_net axi_cpu_interconnect_M00_AXI [get_bd_intf_pins axi_cpu_interconnect/M00_AXI] [get_bd_intf_pins axi_iic_main/S_AXI]
  connect_bd_intf_net -intf_net axi_cpu_interconnect_M01_AXI [get_bd_intf_pins axi_ad9361/s_axi] [get_bd_intf_pins axi_cpu_interconnect/M01_AXI]
  connect_bd_intf_net -intf_net axi_cpu_interconnect_M02_AXI [get_bd_intf_pins axi_ad9361_adc_dma/s_axi] [get_bd_intf_pins axi_cpu_interconnect/M02_AXI]
  connect_bd_intf_net -intf_net axi_cpu_interconnect_M03_AXI [get_bd_intf_pins axi_ad9361_dac_dma/s_axi] [get_bd_intf_pins axi_cpu_interconnect/M03_AXI]
  connect_bd_intf_net -intf_net axi_cpu_interconnect_M04_AXI [get_bd_intf_pins axi_cpu_interconnect/M04_AXI] [get_bd_intf_pins axi_spi/AXI_LITE]
  connect_bd_intf_net -intf_net axi_cpu_interconnect_M05_AXI [get_bd_intf_pins axi_cpu_interconnect/M05_AXI] [get_bd_intf_pins axi_tdd_0/s_axi]
  connect_bd_intf_net -intf_net axi_cpu_interconnect_M06_AXI [get_bd_intf_pins axi_cpu_interconnect/M06_AXI] [get_bd_intf_pins axi_dmac_audio/s_axi]
  connect_bd_intf_net -intf_net axi_cpu_interconnect_M07_AXI [get_bd_intf_pins axi_cpu_interconnect/M07_AXI] [get_bd_intf_pins axi_gpio_rx/S_AXI]
  connect_bd_intf_net -intf_net axi_cpu_interconnect_M08_AXI [get_bd_intf_pins axi_cpu_interconnect/M08_AXI] [get_bd_intf_pins axi_gpio_tx/S_AXI]
  connect_bd_intf_net -intf_net axi_dmac_0_m_dest_axi [get_bd_intf_pins axi_dmac_audio/m_dest_axi] [get_bd_intf_pins sys_ps7/S_AXI_HP0]
  connect_bd_intf_net -intf_net axi_iic_main_IIC [get_bd_intf_ports iic_main] [get_bd_intf_pins axi_iic_main/IIC]
  connect_bd_intf_net -intf_net axis_data_fifo_0_M_AXIS [get_bd_intf_pins axi_dmac_audio/s_axis] [get_bd_intf_pins rx_data_fifo/M_AXIS]
  connect_bd_intf_net -intf_net burst_gate_0_m_fifo_wr [get_bd_intf_pins axi_ad9361_adc_dma/fifo_wr] [get_bd_intf_pins burst_gate/m_fifo_wr]
  connect_bd_intf_net -intf_net cic_compiler_0_M_AXIS_DATA [get_bd_intf_pins rx_cic_q/M_AXIS_DATA] [get_bd_intf_pins rx_iq_packer/s_q]
  connect_bd_intf_net -intf_net cic_compiler_1_M_AXIS_DATA [get_bd_intf_pins rx_cic_i/M_AXIS_DATA] [get_bd_intf_pins rx_iq_packer/s_i]
  connect_bd_intf_net -intf_net cpack_packed_fifo_wr [get_bd_intf_pins burst_gate/s_fifo_wr] [get_bd_intf_pins cpack/packed_fifo_wr]
  connect_bd_intf_net -intf_net dds_compiler_0_M_AXIS_DATA [get_bd_intf_pins rx_cmpy/S_AXIS_B] [get_bd_intf_pins rx_dds_compiler/M_AXIS_DATA]
  connect_bd_intf_net -intf_net dds_compiler_1_M_AXIS_DATA [get_bd_intf_pins tx_cmpy/S_AXIS_B] [get_bd_intf_pins tx_dds_compiler/M_AXIS_DATA]
  connect_bd_intf_net -intf_net fir_compiler_0_M_AXIS_DATA [get_bd_intf_pins rx_data_fifo/S_AXIS] [get_bd_intf_pins rx_fir/M_AXIS_DATA]
  connect_bd_intf_net -intf_net iq_packer_0_m [get_bd_intf_pins rx_fir/S_AXIS_DATA] [get_bd_intf_pins rx_iq_packer/m]
  connect_bd_intf_net -intf_net sys_ps7_DDR [get_bd_intf_ports ddr] [get_bd_intf_pins sys_ps7/DDR]
  connect_bd_intf_net -intf_net sys_ps7_FIXED_IO [get_bd_intf_ports fixed_io] [get_bd_intf_pins sys_ps7/FIXED_IO]

  # Create port connections
  connect_bd_net -net GND_1_dout [get_bd_pins GND_1/dout] [get_bd_pins axi_ad9361/tdd_sync] [get_bd_pins sys_concat_intc/In0] [get_bd_pins sys_concat_intc/In1] [get_bd_pins sys_concat_intc/In2] [get_bd_pins sys_concat_intc/In3] [get_bd_pins sys_concat_intc/In4] [get_bd_pins sys_concat_intc/In5] [get_bd_pins sys_concat_intc/In6] [get_bd_pins sys_concat_intc/In7] [get_bd_pins sys_concat_intc/In8] [get_bd_pins sys_concat_intc/In9] [get_bd_pins sys_concat_intc/In10]
  connect_bd_net -net Net [get_bd_pins tx_cmpy/m_axis_dout_tdata] [get_bd_pins tx_i/Din] [get_bd_pins tx_q/Din]
  connect_bd_net -net axi_ad9361_adc_data_i0 [get_bd_pins axi_ad9361/adc_data_i0] [get_bd_pins cpack/fifo_wr_data_0] [get_bd_pins dsp_mux_rx_0/in0_i]
  connect_bd_net -net axi_ad9361_adc_data_i1 [get_bd_pins axi_ad9361/adc_data_i1] [get_bd_pins cpack/fifo_wr_data_2] [get_bd_pins dsp_mux_rx_0/in1_i]
  connect_bd_net -net axi_ad9361_adc_data_q0 [get_bd_pins axi_ad9361/adc_data_q0] [get_bd_pins cpack/fifo_wr_data_1] [get_bd_pins dsp_mux_rx_0/in0_q]
  connect_bd_net -net axi_ad9361_adc_data_q1 [get_bd_pins axi_ad9361/adc_data_q1] [get_bd_pins cpack/fifo_wr_data_3] [get_bd_pins dsp_mux_rx_0/in1_q]
  connect_bd_net -net axi_ad9361_adc_dma_irq [get_bd_pins axi_ad9361_adc_dma/irq] [get_bd_pins sys_concat_intc/In13]
  connect_bd_net -net axi_ad9361_adc_enable_i0 [get_bd_pins axi_ad9361/adc_enable_i0] [get_bd_pins cpack/enable_0]
  connect_bd_net -net axi_ad9361_adc_enable_i1 [get_bd_pins axi_ad9361/adc_enable_i1] [get_bd_pins cpack/enable_2]
  connect_bd_net -net axi_ad9361_adc_enable_q0 [get_bd_pins axi_ad9361/adc_enable_q0] [get_bd_pins cpack/enable_1]
  connect_bd_net -net axi_ad9361_adc_enable_q1 [get_bd_pins axi_ad9361/adc_enable_q1] [get_bd_pins cpack/enable_3]
  connect_bd_net -net axi_ad9361_adc_valid_i0 [get_bd_pins axi_ad9361/adc_valid_i0] [get_bd_pins cpack/fifo_wr_en] [get_bd_pins rx_cmpy/s_axis_a_tvalid]
  connect_bd_net -net axi_ad9361_dac_dma_irq [get_bd_pins axi_ad9361_dac_dma/irq] [get_bd_pins sys_concat_intc/In12]
  connect_bd_net -net axi_ad9361_dac_enable_i0 [get_bd_pins axi_ad9361/dac_enable_i0] [get_bd_pins dsp_mux_tx/dac_valid] [get_bd_pins tx_upack/enable_0]
  connect_bd_net -net axi_ad9361_dac_enable_i1 [get_bd_pins axi_ad9361/dac_enable_i1] [get_bd_pins tx_upack/enable_2]
  connect_bd_net -net axi_ad9361_dac_enable_q0 [get_bd_pins axi_ad9361/dac_enable_q0] [get_bd_pins tx_upack/enable_1]
  connect_bd_net -net axi_ad9361_dac_enable_q1 [get_bd_pins axi_ad9361/dac_enable_q1] [get_bd_pins tx_upack/enable_3]
  connect_bd_net -net axi_ad9361_enable [get_bd_ports enable] [get_bd_pins axi_ad9361/enable]
  connect_bd_net -net axi_ad9361_l_clk [get_bd_pins axi_ad9361/clk] [get_bd_pins axi_ad9361/l_clk] [get_bd_pins axi_ad9361_adc_dma/fifo_wr_clk] [get_bd_pins axi_ad9361_dac_dma/m_axis_aclk] [get_bd_pins axi_cpu_interconnect/M07_ACLK] [get_bd_pins axi_cpu_interconnect/M08_ACLK] [get_bd_pins axi_dmac_audio/s_axis_aclk] [get_bd_pins axi_gpio_rx/s_axi_aclk] [get_bd_pins axi_gpio_tx/s_axi_aclk] [get_bd_pins axi_tdd_0/clk] [get_bd_pins burst_gate/clk] [get_bd_pins cpack/clk] [get_bd_pins dsp_mux_rx_0/clk] [get_bd_pins dsp_mux_tx/clk] [get_bd_pins rst_axi_ad9361_100M/slowest_sync_clk] [get_bd_pins rx_cic_i/aclk] [get_bd_pins rx_cic_q/aclk] [get_bd_pins rx_cmpy/aclk] [get_bd_pins rx_data_fifo/s_axis_aclk] [get_bd_pins rx_dds_compiler/aclk] [get_bd_pins rx_fir/aclk] [get_bd_pins rx_iq_packer/clk] [get_bd_pins tx_cic_i/aclk] [get_bd_pins tx_cic_q/aclk] [get_bd_pins tx_cmpy/aclk] [get_bd_pins tx_dds_compiler/aclk] [get_bd_pins tx_fir/aclk] [get_bd_pins tx_strobe_gen/clk] [get_bd_pins tx_upack/clk]
  connect_bd_net -net axi_ad9361_rst [get_bd_pins ad9361_resetn/Op1] [get_bd_pins axi_ad9361/rst] [get_bd_pins cpack/reset] [get_bd_pins logic_inv/Op1] [get_bd_pins or_tx_reset/Op1]
  connect_bd_net -net axi_ad9361_tx_clk_out [get_bd_ports tx_clk_out] [get_bd_pins axi_ad9361/tx_clk_out]
  connect_bd_net -net axi_ad9361_tx_data_out [get_bd_ports tx_data_out] [get_bd_pins axi_ad9361/tx_data_out]
  connect_bd_net -net axi_ad9361_tx_frame_out [get_bd_ports tx_frame_out] [get_bd_pins axi_ad9361/tx_frame_out]
  connect_bd_net -net axi_ad9361_txnrx [get_bd_ports txnrx] [get_bd_pins axi_ad9361/txnrx]
  connect_bd_net -net axi_dmac_0_irq [get_bd_pins axi_dmac_audio/irq] [get_bd_pins sys_concat_intc/In14]
  connect_bd_net -net axi_gpio_0_gpio_io_o [get_bd_pins axi_gpio_tx/gpio_io_o] [get_bd_pins tx_antenna_ctrl/Din] [get_bd_pins tx_cic_config/Din] [get_bd_pins tx_cic_config_valid/Din] [get_bd_pins tx_dds_valid/Din] [get_bd_pins tx_dsp_enable/Din] [get_bd_pins tx_strobe/Din]
  connect_bd_net -net axi_gpio_dds_gpio2_io_o [get_bd_pins axi_gpio_rx/gpio2_io_o] [get_bd_pins rx_dds_compiler/s_axis_config_tdata]
  connect_bd_net -net axi_gpio_dds_gpio_io_o [get_bd_pins axi_gpio_rx/gpio_io_o] [get_bd_pins burst_enabled/Din] [get_bd_pins burst_trigger/Din] [get_bd_pins cic_config/Din] [get_bd_pins cic_config_valid/Din] [get_bd_pins dds_valid/Din] [get_bd_pins reset/Din] [get_bd_pins rx_antenna_ctrl/Din]
  connect_bd_net -net axi_gpio_tx_gpio2_io_o [get_bd_pins axi_gpio_tx/gpio2_io_o] [get_bd_pins tx_dds_compiler/s_axis_config_tdata]
  connect_bd_net -net axi_iic_main_iic2intc_irpt [get_bd_pins axi_iic_main/iic2intc_irpt] [get_bd_pins sys_concat_intc/In15]
  connect_bd_net -net axi_spi_io0_o [get_bd_ports spi_sdo_o] [get_bd_pins axi_spi/io0_o]
  connect_bd_net -net axi_spi_ip2intc_irpt [get_bd_pins axi_spi/ip2intc_irpt] [get_bd_pins sys_concat_intc/In11]
  connect_bd_net -net axi_spi_sck_o [get_bd_ports spi_clk_o] [get_bd_pins axi_spi/sck_o]
  connect_bd_net -net axi_spi_ss_o [get_bd_ports spi_csn_o] [get_bd_pins axi_spi/ss_o]
  connect_bd_net -net axi_tdd_0_tdd_channel_0 [get_bd_ports txdata_o] [get_bd_pins axi_tdd_0/tdd_channel_0]
  connect_bd_net -net axi_tdd_0_tdd_channel_1 -boundary_type upper [get_bd_pins axi_tdd_0/tdd_channel_1]
  connect_bd_net -net axi_tdd_0_tdd_channel_2 [get_bd_pins axi_tdd_0/tdd_channel_2] [get_bd_pins or_tx_reset/Op2]
  connect_bd_net -net burst_enabled_Dout [get_bd_pins burst_enabled/Dout] [get_bd_pins burst_gate/enabled]
  connect_bd_net -net burst_gate_m_fifo_wr_sync [get_bd_pins axi_ad9361_adc_dma/fifo_wr_sync] [get_bd_pins burst_gate/m_fifo_wr_sync]
  connect_bd_net -net burst_trigger_Dout [get_bd_pins burst_gate/trigger] [get_bd_pins burst_trigger/Dout]
  connect_bd_net -net cic_compiler_2_m_axis_data_tdata [get_bd_pins tx_cic_data/In0] [get_bd_pins tx_cic_i/m_axis_data_tdata]
  connect_bd_net -net cic_compiler_2_m_axis_data_tvalid [get_bd_pins tx_cic_i/m_axis_data_tvalid] [get_bd_pins tx_cmpy/s_axis_a_tvalid]
  connect_bd_net -net cic_compiler_3_m_axis_data_tdata [get_bd_pins tx_cic_data/In1] [get_bd_pins tx_cic_q/m_axis_data_tdata]
  connect_bd_net -net cpack_fifo_wr_overflow [get_bd_pins axi_ad9361/adc_dovf] [get_bd_pins cpack/fifo_wr_overflow]
  connect_bd_net -net dsp_mux_rx_0_out_i [get_bd_pins dsp_mux_rx_0/out_i] [get_bd_pins rx_raw_data/In0]
  connect_bd_net -net dsp_mux_rx_0_out_q [get_bd_pins dsp_mux_rx_0/out_q] [get_bd_pins rx_raw_data/In1]
  connect_bd_net -net dsp_mux_tx_0_out_i0 [get_bd_pins axi_ad9361/dac_data_i0] [get_bd_pins dsp_mux_tx/dac_i0]
  connect_bd_net -net dsp_mux_tx_0_out_q0 [get_bd_pins axi_ad9361/dac_data_q0] [get_bd_pins dsp_mux_tx/dac_q0]
  connect_bd_net -net dsp_mux_tx_0_proc_i [get_bd_pins dsp_mux_tx/proc_i] [get_bd_pins tx_raw_data/In0]
  connect_bd_net -net dsp_mux_tx_0_proc_q [get_bd_pins dsp_mux_tx/proc_q] [get_bd_pins tx_raw_data/In1]
  connect_bd_net -net dsp_mux_tx_dac_i1 [get_bd_pins axi_ad9361/dac_data_i1] [get_bd_pins dsp_mux_tx/dac_i1]
  connect_bd_net -net dsp_mux_tx_dac_q1 [get_bd_pins axi_ad9361/dac_data_q1] [get_bd_pins dsp_mux_tx/dac_q1]
  connect_bd_net -net dsp_mux_tx_fifo_rd_en [get_bd_pins dsp_mux_tx/fifo_rd_en] [get_bd_pins tx_upack/fifo_rd_en]
  connect_bd_net -net dsp_mux_tx_proc_valid [get_bd_pins dsp_mux_tx/proc_valid] [get_bd_pins tx_fir/s_axis_data_tvalid]
  connect_bd_net -net fir_compiler_1_m_axis_data_tdata [get_bd_pins tx_fir/m_axis_data_tdata] [get_bd_pins tx_fir_i/Din] [get_bd_pins tx_fir_q/Din]
  connect_bd_net -net fir_compiler_1_m_axis_data_tvalid [get_bd_pins tx_cic_i/s_axis_data_tvalid] [get_bd_pins tx_cic_q/s_axis_data_tvalid] [get_bd_pins tx_fir/m_axis_data_tvalid]
  connect_bd_net -net gpio_i_1 [get_bd_ports gpio_i] [get_bd_pins sys_ps7/GPIO_I]
  connect_bd_net -net logic_inv_Res [get_bd_pins axi_tdd_0/resetn] [get_bd_pins logic_inv/Res]
  connect_bd_net -net logic_or_1_Res [get_bd_pins or_tx_reset/Res] [get_bd_pins tx_upack/reset]
  connect_bd_net -net mixer_mode_Dout [get_bd_pins cic_config_valid/Dout] [get_bd_pins rx_cic_i/s_axis_config_tvalid] [get_bd_pins rx_cic_q/s_axis_config_tvalid]
  connect_bd_net -net reset_Dout [get_bd_pins reset/Dout] [get_bd_pins rx_cic_i/aresetn] [get_bd_pins rx_cic_q/aresetn] [get_bd_pins rx_cmpy/aresetn] [get_bd_pins rx_data_fifo/s_axis_aresetn] [get_bd_pins rx_dds_compiler/aresetn] [get_bd_pins rx_fir/aresetn] [get_bd_pins rx_iq_packer/resetn]
  connect_bd_net -net rst_axi_ad9361_100M_peripheral_aresetn [get_bd_pins axi_cpu_interconnect/M07_ARESETN] [get_bd_pins axi_cpu_interconnect/M08_ARESETN] [get_bd_pins axi_gpio_rx/s_axi_aresetn] [get_bd_pins axi_gpio_tx/s_axi_aresetn] [get_bd_pins rst_axi_ad9361_100M/peripheral_aresetn]
  connect_bd_net -net rx_antenna_ctrl_Dout [get_bd_pins dsp_mux_rx_0/sel] [get_bd_pins rx_antenna_ctrl/Dout]
  connect_bd_net -net rx_clk_in_1 [get_bd_ports rx_clk_in] [get_bd_pins axi_ad9361/rx_clk_in]
  connect_bd_net -net rx_cmpy_m_axis_dout_tdata [get_bd_pins rx_cmpy/m_axis_dout_tdata] [get_bd_pins rx_cmpy_i/Din] [get_bd_pins rx_cmpy_q/Din]
  connect_bd_net -net rx_cmpy_m_axis_dout_tvalid [get_bd_pins rx_cic_i/s_axis_data_tvalid] [get_bd_pins rx_cic_q/s_axis_data_tvalid] [get_bd_pins rx_cmpy/m_axis_dout_tvalid]
  connect_bd_net -net rx_data_in_1 [get_bd_ports rx_data_in] [get_bd_pins axi_ad9361/rx_data_in]
  connect_bd_net -net rx_frame_in_1 [get_bd_ports rx_frame_in] [get_bd_pins axi_ad9361/rx_frame_in]
  connect_bd_net -net spi0_clk_i_1 [get_bd_ports spi0_clk_i] [get_bd_pins sys_ps7/SPI0_SCLK_I]
  connect_bd_net -net spi0_csn_i_1 [get_bd_ports spi0_csn_i] [get_bd_pins sys_ps7/SPI0_SS_I]
  connect_bd_net -net spi0_sdi_i_1 [get_bd_ports spi0_sdi_i] [get_bd_pins sys_ps7/SPI0_MISO_I]
  connect_bd_net -net spi0_sdo_i_1 [get_bd_ports spi0_sdo_i] [get_bd_pins sys_ps7/SPI0_MOSI_I]
  connect_bd_net -net spi_clk_i_1 [get_bd_ports spi_clk_i] [get_bd_pins axi_spi/sck_i]
  connect_bd_net -net spi_csn_i_1 [get_bd_ports spi_csn_i] [get_bd_pins axi_spi/ss_i]
  connect_bd_net -net spi_sdi_i_1 [get_bd_ports spi_sdi_i] [get_bd_pins axi_spi/io1_i]
  connect_bd_net -net spi_sdo_i_1 [get_bd_ports spi_sdo_i] [get_bd_pins axi_spi/io0_i]
  connect_bd_net -net sync_in_1 [get_bd_ports tdd_ext_sync] [get_bd_pins axi_tdd_0/sync_in]
  connect_bd_net -net sys_200m_clk [get_bd_pins axi_ad9361/delay_clk] [get_bd_pins sys_ps7/FCLK_CLK1]
  connect_bd_net -net sys_concat_intc_dout [get_bd_pins sys_concat_intc/dout] [get_bd_pins sys_ps7/IRQ_F2P]
  connect_bd_net -net sys_cpu_clk [get_bd_pins axi_ad9361/s_axi_aclk] [get_bd_pins axi_ad9361_adc_dma/m_dest_axi_aclk] [get_bd_pins axi_ad9361_adc_dma/s_axi_aclk] [get_bd_pins axi_ad9361_dac_dma/m_src_axi_aclk] [get_bd_pins axi_ad9361_dac_dma/s_axi_aclk] [get_bd_pins axi_cpu_interconnect/ACLK] [get_bd_pins axi_cpu_interconnect/M00_ACLK] [get_bd_pins axi_cpu_interconnect/M01_ACLK] [get_bd_pins axi_cpu_interconnect/M02_ACLK] [get_bd_pins axi_cpu_interconnect/M03_ACLK] [get_bd_pins axi_cpu_interconnect/M04_ACLK] [get_bd_pins axi_cpu_interconnect/M05_ACLK] [get_bd_pins axi_cpu_interconnect/M06_ACLK] [get_bd_pins axi_cpu_interconnect/S00_ACLK] [get_bd_pins axi_cpu_interconnect/S01_ACLK] [get_bd_pins axi_dmac_audio/m_dest_axi_aclk] [get_bd_pins axi_dmac_audio/s_axi_aclk] [get_bd_pins axi_iic_main/s_axi_aclk] [get_bd_pins axi_spi/ext_spi_clk] [get_bd_pins axi_spi/s_axi_aclk] [get_bd_pins axi_tdd_0/s_axi_aclk] [get_bd_pins sys_ps7/FCLK_CLK0] [get_bd_pins sys_ps7/M_AXI_GP0_ACLK] [get_bd_pins sys_ps7/S_AXI_HP0_ACLK] [get_bd_pins sys_ps7/S_AXI_HP1_ACLK] [get_bd_pins sys_ps7/S_AXI_HP2_ACLK] [get_bd_pins sys_rstgen/slowest_sync_clk]
  connect_bd_net -net sys_cpu_reset [get_bd_pins sys_rstgen/peripheral_reset]
  connect_bd_net -net sys_cpu_resetn [get_bd_pins axi_ad9361/s_axi_aresetn] [get_bd_pins axi_ad9361_adc_dma/m_dest_axi_aresetn] [get_bd_pins axi_ad9361_adc_dma/s_axi_aresetn] [get_bd_pins axi_ad9361_dac_dma/m_src_axi_aresetn] [get_bd_pins axi_ad9361_dac_dma/s_axi_aresetn] [get_bd_pins axi_cpu_interconnect/ARESETN] [get_bd_pins axi_cpu_interconnect/M00_ARESETN] [get_bd_pins axi_cpu_interconnect/M01_ARESETN] [get_bd_pins axi_cpu_interconnect/M02_ARESETN] [get_bd_pins axi_cpu_interconnect/M03_ARESETN] [get_bd_pins axi_cpu_interconnect/M04_ARESETN] [get_bd_pins axi_cpu_interconnect/M05_ARESETN] [get_bd_pins axi_cpu_interconnect/M06_ARESETN] [get_bd_pins axi_cpu_interconnect/S00_ARESETN] [get_bd_pins axi_cpu_interconnect/S01_ARESETN] [get_bd_pins axi_dmac_audio/m_dest_axi_aresetn] [get_bd_pins axi_dmac_audio/s_axi_aresetn] [get_bd_pins axi_iic_main/s_axi_aresetn] [get_bd_pins axi_spi/s_axi_aresetn] [get_bd_pins axi_tdd_0/s_axi_aresetn] [get_bd_pins sys_rstgen/peripheral_aresetn]
  connect_bd_net -net sys_ps7_FCLK_RESET0_N [get_bd_pins rst_axi_ad9361_100M/ext_reset_in] [get_bd_pins sys_ps7/FCLK_RESET0_N] [get_bd_pins sys_rstgen/ext_reset_in]
  connect_bd_net -net sys_ps7_GPIO_O [get_bd_ports gpio_o] [get_bd_pins sys_ps7/GPIO_O]
  connect_bd_net -net sys_ps7_GPIO_T [get_bd_ports gpio_t] [get_bd_pins sys_ps7/GPIO_T]
  connect_bd_net -net sys_ps7_SPI0_MOSI_O [get_bd_ports spi0_sdo_o] [get_bd_pins sys_ps7/SPI0_MOSI_O]
  connect_bd_net -net sys_ps7_SPI0_SCLK_O [get_bd_ports spi0_clk_o] [get_bd_pins sys_ps7/SPI0_SCLK_O]
  connect_bd_net -net sys_ps7_SPI0_SS1_O [get_bd_ports spi0_csn_1_o] [get_bd_pins sys_ps7/SPI0_SS1_O]
  connect_bd_net -net sys_ps7_SPI0_SS2_O [get_bd_ports spi0_csn_2_o] [get_bd_pins sys_ps7/SPI0_SS2_O]
  connect_bd_net -net sys_ps7_SPI0_SS_O [get_bd_ports spi0_csn_0_o] [get_bd_pins sys_ps7/SPI0_SS_O]
  connect_bd_net -net tx_antenna_ctrl_Dout [get_bd_pins dsp_mux_tx/sel] [get_bd_pins tx_antenna_ctrl/Dout]
  connect_bd_net -net tx_cic_config_valid_Dout [get_bd_pins tx_cic_config_valid/Dout] [get_bd_pins tx_cic_i/s_axis_config_tvalid] [get_bd_pins tx_cic_q/s_axis_config_tvalid]
  connect_bd_net -net tx_cic_i_s_axis_data_tready [get_bd_pins tx_cic_i/s_axis_data_tready] [get_bd_pins tx_fir/m_axis_data_tready]
  connect_bd_net -net tx_cic_valid_Dout [get_bd_pins tx_dds_compiler/s_axis_config_tvalid] [get_bd_pins tx_dds_valid/Dout]
  connect_bd_net -net tx_dsp_enable_Dout [get_bd_pins dsp_mux_tx/enabled] [get_bd_pins tx_dsp_enable/Dout]
  connect_bd_net -net tx_fir_mode_Dout [get_bd_pins tx_strobe/Dout] [get_bd_pins tx_strobe_gen/phase_inc]
  connect_bd_net -net tx_sample_enable_0_enable [get_bd_pins dsp_mux_tx/strobe_in] [get_bd_pins tx_cic_i/m_axis_data_tready] [get_bd_pins tx_cic_q/m_axis_data_tready] [get_bd_pins tx_strobe_gen/strobe_out]
  connect_bd_net -net tx_upack_fifo_rd_data_0 [get_bd_pins dsp_mux_tx/dma_i0] [get_bd_pins tx_upack/fifo_rd_data_0]
  connect_bd_net -net tx_upack_fifo_rd_data_1 [get_bd_pins dsp_mux_tx/dma_q0] [get_bd_pins tx_upack/fifo_rd_data_1]
  connect_bd_net -net tx_upack_fifo_rd_data_2 [get_bd_pins dsp_mux_tx/dma_i1] [get_bd_pins tx_upack/fifo_rd_data_2]
  connect_bd_net -net tx_upack_fifo_rd_data_3 [get_bd_pins dsp_mux_tx/dma_q1] [get_bd_pins tx_upack/fifo_rd_data_3]
  connect_bd_net -net tx_upack_fifo_rd_underflow [get_bd_pins axi_ad9361/dac_dunf] [get_bd_pins tx_upack/fifo_rd_underflow]
  connect_bd_net -net tx_upack_fifo_rd_valid [get_bd_pins dsp_mux_tx/dma_valid] [get_bd_pins tx_upack/fifo_rd_valid]
  connect_bd_net -net up_enable_1 [get_bd_ports up_enable] [get_bd_pins axi_ad9361/up_enable]
  connect_bd_net -net up_txnrx_1 [get_bd_ports up_txnrx] [get_bd_pins axi_ad9361/up_txnrx]
  connect_bd_net -net util_vector_logic_0_Res [get_bd_pins ad9361_resetn/Res] [get_bd_pins burst_gate/rst_n]
  connect_bd_net -net xlconcat_0_dout [get_bd_pins rx_cmpy/s_axis_a_tdata] [get_bd_pins rx_raw_data/dout]
  connect_bd_net -net xlconcat_1_dout [get_bd_pins tx_cic_data/dout] [get_bd_pins tx_cmpy/s_axis_a_tdata]
  connect_bd_net -net xlconcat_tx_fir_dout [get_bd_pins tx_fir/s_axis_data_tdata] [get_bd_pins tx_raw_data/dout]
  connect_bd_net -net xlslice_0_Dout [get_bd_pins rx_cic_i/s_axis_data_tdata] [get_bd_pins rx_cmpy_i/Dout]
  connect_bd_net -net xlslice_1_Dout [get_bd_pins rx_cic_q/s_axis_data_tdata] [get_bd_pins rx_cmpy_q/Dout]
  connect_bd_net -net xlslice_2_Dout [get_bd_pins cic_config/Dout] [get_bd_pins rx_cic_i/s_axis_config_tdata] [get_bd_pins rx_cic_q/s_axis_config_tdata] [get_bd_pins rx_iq_packer/dec_rate]
  connect_bd_net -net xlslice_2_Dout1 [get_bd_pins dds_valid/Dout] [get_bd_pins rx_dds_compiler/s_axis_config_tvalid]
  connect_bd_net -net xlslice_3_Dout [get_bd_pins dsp_mux_tx/cic_interp] [get_bd_pins tx_cic_config/Dout] [get_bd_pins tx_cic_i/s_axis_config_tdata] [get_bd_pins tx_cic_q/s_axis_config_tdata]
  connect_bd_net -net xlslice_4_Dout [get_bd_pins dsp_mux_tx/modulated_i] [get_bd_pins tx_i/Dout]
  connect_bd_net -net xlslice_5_Dout [get_bd_pins dsp_mux_tx/modulated_q] [get_bd_pins tx_q/Dout]
  connect_bd_net -net xlslice_tx_fir_i_Dout [get_bd_pins tx_cic_i/s_axis_data_tdata] [get_bd_pins tx_fir_i/Dout]
  connect_bd_net -net xlslice_tx_fir_q_Dout [get_bd_pins tx_cic_q/s_axis_data_tdata] [get_bd_pins tx_fir_q/Dout]

  # Create address segments
  assign_bd_address -offset 0x00000000 -range 0x20000000 -target_address_space [get_bd_addr_spaces axi_ad9361_adc_dma/m_dest_axi] [get_bd_addr_segs sys_ps7/S_AXI_HP1/HP1_DDR_LOWOCM] -force
  assign_bd_address -offset 0x00000000 -range 0x20000000 -target_address_space [get_bd_addr_spaces axi_ad9361_dac_dma/m_src_axi] [get_bd_addr_segs sys_ps7/S_AXI_HP2/HP2_DDR_LOWOCM] -force
  assign_bd_address -offset 0x00000000 -range 0x20000000 -target_address_space [get_bd_addr_spaces axi_dmac_audio/m_dest_axi] [get_bd_addr_segs sys_ps7/S_AXI_HP0/HP0_DDR_LOWOCM] -force
  assign_bd_address -offset 0x40400000 -range 0x00010000 -target_address_space [get_bd_addr_spaces sys_ps7/Data] [get_bd_addr_segs axi_dmac_audio/s_axi/axi_lite] -force
  assign_bd_address -offset 0x41210000 -range 0x00010000 -target_address_space [get_bd_addr_spaces sys_ps7/Data] [get_bd_addr_segs axi_gpio_rx/S_AXI/Reg] -force
  assign_bd_address -offset 0x41200000 -range 0x00010000 -target_address_space [get_bd_addr_spaces sys_ps7/Data] [get_bd_addr_segs axi_gpio_tx/S_AXI/Reg] -force
  assign_bd_address -offset 0x79020000 -range 0x00010000 -target_address_space [get_bd_addr_spaces sys_ps7/Data] [get_bd_addr_segs axi_ad9361/s_axi/axi_lite] -force
  assign_bd_address -offset 0x7C400000 -range 0x00001000 -target_address_space [get_bd_addr_spaces sys_ps7/Data] [get_bd_addr_segs axi_ad9361_adc_dma/s_axi/axi_lite] -force
  assign_bd_address -offset 0x7C420000 -range 0x00001000 -target_address_space [get_bd_addr_spaces sys_ps7/Data] [get_bd_addr_segs axi_ad9361_dac_dma/s_axi/axi_lite] -force
  assign_bd_address -offset 0x41600000 -range 0x00001000 -target_address_space [get_bd_addr_spaces sys_ps7/Data] [get_bd_addr_segs axi_iic_main/S_AXI/Reg] -force
  assign_bd_address -offset 0x7C430000 -range 0x00001000 -target_address_space [get_bd_addr_spaces sys_ps7/Data] [get_bd_addr_segs axi_spi/AXI_LITE/Reg] -force
  assign_bd_address -offset 0x7C440000 -range 0x00001000 -target_address_space [get_bd_addr_spaces sys_ps7/Data] [get_bd_addr_segs axi_tdd_0/tdd_core/s_axi/axi_lite] -force


  # Restore current instance
  current_bd_instance $oldCurInst

  validate_bd_design
  save_bd_design
}
# End of create_root_design()


##################################################################
# MAIN FLOW
##################################################################

create_root_design ""


