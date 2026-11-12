/* AT32F405CCT7: 256 KB flash, 96 KB SRAM (102 KB with parity checking off; 96 KB is the safe common size). */
MEMORY
{
  FLASH : ORIGIN = 0x08000000, LENGTH = 256K
  RAM   : ORIGIN = 0x20000000, LENGTH = 96K
}
