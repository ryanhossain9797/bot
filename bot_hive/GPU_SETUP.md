# Full Guide: Proxmox GPU Passthrough for Docker & LLMs

This guide provides a comprehensive, step-by-step process for passing an NVIDIA GPU from a Proxmox host to an Ubuntu VM, and then into a Docker container for GPU-accelerated applications like `llama.cpp`. Each section includes explicit verification steps to ensure success.

## Layer 1: Proxmox Host Configuration (GPU Passthrough)

The goal here is to completely detach the GPU from the Proxmox host so it can be exclusively used by the VM.

### 1.1: Configure BIOS/UEFI

Reboot your Proxmox host and enter the system's BIOS/UEFI setup. Enable the following options:

*   **Virtualization Technology:** `Intel VT-d` or `AMD-Vi`
*   **IOMMU:** `IOMMU`, `VT-d`, or `AMD-Vi`
*   **Above 4G Decoding:** Enabled
*   **Primary Display:** Set to `Integrated Graphics` (if available) to prevent Proxmox from using your NVIDIA card for its own console.
*   **CSM / Legacy Boot:** Disabled (if Proxmox is installed in UEFI mode).

### 1.2: Configure GRUB for IOMMU

SSH into your Proxmox host to modify the kernel boot parameters.

1.  **Edit GRUB:**
    ```bash
    nano /etc/default/grub
    ```

2.  **Modify the `GRUB_CMDLINE_LINUX_DEFAULT` line.**
    *   **For Intel CPUs:**
        ```
        GRUB_CMDLINE_LINUX_DEFAULT="quiet intel_iommu=on iommu=pt pcie_acs_override=downstream,multifunction nofb nomodeset video=vesafb:off,efifb:off"
        ```
    *   **For AMD CPUs:**
        ```
        GRUB_CMDLINE_LINUX_DEFAULT="quiet amd_iommu=on iommu=pt pcie_acs_override=downstream,multifunction nofb nomodeset video=vesafb:off,efifb:off"
        ```

3.  **Update GRUB and Reboot:**
    ```bash
    update-grub
    sudo reboot
    ```

### 1.3: Verify IOMMU Activation

After rebooting, SSH back into your host and verify that IOMMU is enabled.

```bash
dmesg | grep -e DMAR -e IOMMU
```
You should see output indicating that IOMMU is enabled (e.g., "DMAR: IOMMU enabled"). If this command produces no output, review your BIOS and GRUB settings.

### 1.4: Configure VFIO Modules

Configure the `vfio` modules that will take control of the GPU.

1.  **Load VFIO modules on boot:**
    ```bash
    nano /etc/modules
    ```
    Add the following lines:
    ```
    vfio
    vfio_iommu_type1
    vfio_pci
    vfio_virqfd
    ```

2.  **Blacklist NVIDIA drivers** to prevent the host from loading them:
    ```bash
    nano /etc/modprobe.d/blacklist.conf
    ```
    Add these lines:
    ```
    blacklist nouveau
    blacklist nvidia
    blacklist nvidiafb
    ```

3.  **Identify your GPU's PCI IDs:**
    ```bash
    lspci -nn | grep -i nvidia
    ```
    Note the two `[vendor:device]` IDs (e.g., `10de:1f08` and `10de:10f9`).

4.  **Assign the GPU to `vfio-pci`:**
    ```bash
    nano /etc/modprobe.d/vfio.conf
    ```
    Add a line with your specific IDs:
    ```
    options vfio-pci ids=10de:1f08,10de:10f9 disable_vga=1
    ```

5.  **Update the initial RAM disk and reboot:**
    ```bash
    update-initramfs -u
    sudo reboot
    ```

### 1.5: Verify VFIO Driver Binding

After the final host reboot, confirm that the `vfio-pci` driver has claimed the GPU. Replace `01:00.0` with your GPU's ID from `lspci`.

```bash
lspci -k -s 01:00.0
```
The output **must** include the line: `Kernel driver in use: vfio-pci`. If it shows `nouveau` or `nvidia`, the blacklisting failed. If it's empty, the binding failed.

## Layer 2: Ubuntu VM Configuration

### 2.1: Create and Configure the VM

In the Proxmox Web UI, create a new VM with these specific settings:
*   **OS:** Ubuntu Server 22.04 or newer.
*   **Machine:** `q35`
*   **BIOS:** `OVMF (UEFI)`
*   **CPU:** Set `Type` to `host`.
*   **Memory:** Disable the `Ballooning Device`.

### 2.2: Pass the GPU and Verify Visibility

1.  With the VM **powered off**, go to its **Hardware** tab in Proxmox.
2.  Click **Add** -> **PCI Device**.
3.  Select your NVIDIA GPU from the dropdown menu.
4.  Check the boxes for **All Functions**, **ROM-Bar**, and **PCI-Express**.
5.  Go to the VM's `Display` setting and change it to `None`.
6.  **Start the Ubuntu VM.**

7.  **Verify Hardware Visibility in VM:** Before installing any drivers, log into your VM and run:
    ```bash
    lspci | grep -i nvidia
    ```
    You should see your NVIDIA GPU listed. If not, the passthrough from Proxmox has failed.

### 2.3: Install NVIDIA Drivers and Verify

1.  Install the latest NVIDIA drivers from Ubuntu's repositories.
    ```bash
    sudo apt update
    sudo apt install nvidia-driver-550 # Or a newer version if available
    sudo reboot
    ```
2.  **Verify Driver Installation in VM:** This is the most important check inside the VM.
    ```bash
    nvidia-smi
    ```
    If this command shows your GPU's details (model, driver version, CUDA version), the VM is correctly configured.

## Layer 3: Docker Configuration (Inside the Ubuntu VM)

### 3.1: Install the NVIDIA Container Toolkit

This toolkit acts as the bridge between the NVIDIA drivers on the VM and your Docker containers.

```bash
# Add the NVIDIA repository GPG key
curl -fsSL https://nvidia.github.io/libnvidia-container/gpgkey | sudo gpg --dearmor -o /usr/share/keyrings/nvidia-container-toolkit-keyring.gpg

# Add the repository itself
curl -s -L https://nvidia.github.io/libnvidia-container/$(. /etc/os-release;echo $ID$VERSION_ID)/nvidia-container-toolkit.list | sudo tee /etc/apt/sources.list.d/nvidia-container-toolkit.list

# Install the toolkit
sudo apt-get update
sudo apt-get install -y nvidia-container-toolkit

# Configure Docker to use the NVIDIA runtime and restart
sudo nvidia-ctk runtime configure --runtime=docker
sudo systemctl restart docker
```

### 3.2: Verify Docker GPU Access

Confirm that Docker can access the GPU by running a test container.

```bash
docker run --rm --gpus all nvidia/cuda:12.1.0-base-ubuntu22.04 nvidia-smi
```
This should again show your GPU details, but this time from *within a container*. If this works, your entire setup is successful.

### 3.3: Run your Application

You can now run your `bot_hive` application with GPU acceleration.

```bash
docker run --gpus all your-bot-hive-image
```

Your `bot_hive` `Dockerfile` should be built `FROM` an appropriate NVIDIA CUDA development image to ensure the `llama-cpp-2` crate can be compiled with CUDA support.
