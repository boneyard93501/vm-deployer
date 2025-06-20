# Fluence VM Deployer

A high-performance Rust tool for deploying and benchmarking virtual machines on the Fluence network with parallel execution and comprehensive analytics.

## Features

- **Parallel VM Deployment**: Deploy VMs using both bulk and decomposed approaches simultaneously
- **Real-time Status Monitoring**: Track VM launch phases from request to active status
- **Provider Information**: Log VM provider, location, and datacenter details
- **Comprehensive Analytics**: Success rates, timing comparisons, and performance metrics
- **Automatic Cleanup**: Delete VMs after deployment with configurable delays
- **Signal Handling**: Graceful shutdown with emergency VM cleanup on Ctrl+C
- **Per-run Logging**: Separate log files for each deployment run in logs/ directory

## Quick Start

### 1. Installation

```bash
git clone <repository>
cd vm-deployer
cargo build --release
```

### 2. Configuration

Create your `.env` file with secrets:

```bash
# Required: Your Fluence API key
FLUENCE_API_KEY=your_api_key_here

# Required: SSH public keys (comma-separated)
SSH_KEYS=ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAICFgwL0PUvNS1APQvX0Ilt7PG5e3wFIEXf6v/E1c45Tp user@host

# Optional: Log file path (defaults to ./logs/vm_deployment.log)
LOG_FILE_PATH=./logs/vm_deployment.log
```

Create your `deployment_config.yaml`:

```yaml
# Application Configuration
apiBaseUrl: "https://api.fluence.dev"
configFilePath: "./deployment_config.yaml"
enableConsoleOutput: true

# VM Status Polling Configuration
deploymentTimeoutSeconds: 600      # Max time to wait for VM to be Active
pollIntervalSeconds: 30            # How often to check VM status
enableStatusPolling: true          # Whether to wait for Active status
enableIpRetrieval: true            # Whether to get IP addresses

# HTTP Client Configuration
httpTimeoutSeconds: 120            # HTTP request timeout (2 minutes)

# VM Cleanup Configuration
batchDeleteEnabled: true          # Whether to delete VMs after deployment
deleteDelaySeconds: 0             # Delay before deletion (0 = immediate)

# Deployment Approach Configuration
enableBulkApproach: true          # One API call with instances: N
enableDecomposedApproach: true    # N separate API calls

deployments:
  - vmConfig:
      name: "test-vm"
      basicConfiguration: "cpu-2-ram-4gb-storage-25gb"
      openPorts:
        - port: 22
          protocol: "tcp"
        - port: 8080
          protocol: "tcp"
      hostname: "test-vm"
      osImage: "https://cloud-images.ubuntu.com/focal/current/focal-server-cloudimg-amd64.img"
      sshKeys: []  # Will use SSH keys from .env file if empty
    datacenter:
      countries: ["US", "CA", "DE", "FR", "GB"]
    instances: 3
    maxPricePerEpochUsd: 2.0
```

### 3. Run Deployment

```bash
cargo run --release
```

## Configuration Options

### Deployment Approaches

- **Bulk Approach**: Single API call creating multiple VMs simultaneously
- **Decomposed Approach**: Separate API calls for each VM with unique naming

Enable/disable either approach in `deployment_config.yaml`:

```yaml
enableBulkApproach: true          # Enable bulk deployment
enableDecomposedApproach: true    # Enable decomposed deployment
```

### VM Configuration

```yaml
deployments:
  - vmConfig:
      name: "my-vm"                                    # VM name
      basicConfiguration: "cpu-2-ram-4gb-storage-25gb" # Resource spec
      openPorts:                                       # Network ports
        - port: 22
          protocol: "tcp"
      hostname: "my-vm"                               # VM hostname
      osImage: "https://ubuntu-image-url"             # OS image URL
      sshKeys: []                                     # SSH keys (or use .env)
    datacenter:
      countries: ["US", "CA"]                         # Preferred countries
    instances: 3                                      # Number of VMs
    maxPricePerEpochUsd: 1.5                         # Price limit
```

### Timeout and Polling

```yaml
deploymentTimeoutSeconds: 600      # VM launch timeout
pollIntervalSeconds: 30            # Status check frequency
enableStatusPolling: true          # Wait for Active status
enableIpRetrieval: true            # Require IP address
```

## Output and Logging

### Console Output

The tool provides real-time console output showing:

- Configuration summary
- VM request progress
- Launch phase transitions
- Provider and location information
- Performance analytics

### Log Files

Each run creates a timestamped log file in the `logs/` directory:

```
logs/vm_deployment_20241219_143022.log
```

Log files contain:
- Complete configuration used
- VM status transitions with timing
- Provider and datacenter information
- Success/failure analysis
- Cleanup operations

### Sample Output

```
✓ LAUNCHED: test-vm-1 [0xABC123...] (DECOMPOSED) IP: 82.177.167.169 Datacenter: us-east-1
✗ FAILED:   test-vm-2 [0xDEF456...] (BULK) - Status: TIMEOUT

PERFORMANCE COMPARISON:
  Bulk: 1/3 VMs launched (33.3% success rate)
  Decomposed: 3/3 VMs launched (100.0% success rate)
  Speedup Factor: 1.88x (Decomposed faster)
```

## Advanced Usage

### Signal Handling

Press Ctrl+C for graceful shutdown:
- Stops creating new VMs
- Waits for active deployments to complete
- Deletes any created VMs
- Logs cleanup operations

### Provider Analysis

The tool logs provider information for each VM:

```
[POLLING] VM test-vm-1 FULLY LAUNCHED in 180.5s IP: 82.177.167.169 Datacenter: us-east-1
```

### Performance Benchmarking

Compare deployment approaches:

- **Success Rates**: Which approach has higher success rates
- **Timing Analysis**: API request time vs full launch time
- **Individual VM Metrics**: Per-VM timing for decomposed approach
- **Provider Distribution**: Which providers/locations are used

## Troubleshooting

### Common Issues

1. **VMs stuck in "Launching"**: Increase `deploymentTimeoutSeconds`
2. **Rate limiting**: Increase `pollIntervalSeconds`
3. **All VMs deploy to same region**: Check `countries` configuration in `datacenter` section
4. **No SSH access**: Verify SSH keys in `.env` file

### Debug Information

Enable debug output by checking the console for:

- JSON request/response dumps
- Location constraint verification
- VM status polling progress
- Channel communication tracking

### Log Analysis

Check the timestamped log files in `logs/` directory for:
- Complete deployment timeline
- VM-by-VM success/failure details
- Provider and location distribution
- Performance metrics and comparisons

## Dependencies

- Rust 2024 edition
- `reqwest` - HTTP client
- `serde` - JSON/YAML serialization
- `chrono` - Timestamp handling
- `ctrlc` - Signal handling
