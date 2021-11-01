# ClockBound Protocol Version 1

## Request
### Request Header

| 0 | 1 | 2 | 3 |
|---|---|---|---|
| V | T |RSV|RSV|

V, u8: The protocol version of the request (1).  
T, u8: The request type: Now (1), Before (2), After (3).  
RSV, u8: Reserved.  
RSV, u8: Reserved.

### Now Request

|0 1 2 3 |
|:------:|
|HEADER  |

HEADER: See header definition above. Now request only has the header. T set to Now (1).

### Before/After Request
| 0  1  2  3 | 4 5 6 7 8 9 10 11 |
|:----------:|:-----------------:|
|HEADER      |EPOCH              |

HEADER: See header defintion above. T set to either Before (2) or After (3).  
EPOCH, u64: The time we are testing against represented as the number of nanoseconds from the unix epoch (Jan 1 1970 UTC)

## Response
### Response Header
| 0 | 1 | 2 | 3 |
|---|---|---|---|
| V | T | F |RSV|

V, u8: The protocol version of this response.  
T, u8: The response type. Should always match a valid request type; otherwise returns Error (0).  
F, u8: Set to 1 if Chrony is not synchronized. Set to 0 otherwise.  
RSV, u8: Reserved.

### Now Response
| 0  1  2  3 | 4  5  6  7  8  9  10 11| 12 13 14 15 16 17 18 19|
|:----------:|:----------------------:|:----------------------:|
|HEADER      |EARLIEST                |LATEST                  |

HEADER: See header definition above.  
EARLIEST, u64: Clock Time - Clock Error Bound represented as the number of nanoseconds from the unix epoch (Jan 1 1970 UTC).  
LATEST, u64: Clock Time + Clock Error Bound represented as the number of nanoseconds from the unix epoch (Jan 1 1970 UTC).

### After Response
| 0  1  2  3 | 4   |
|:----------:|:---:|
|HEADER      |A    |

A, u8: Set to 1 (true) if the requested time happened after the latest error bound of the current system time, otherwise 0 (false).

### Before Response
| 0  1  2  3 | 4   |
|:----------:|:---:|
|HEADER      |B    |

B, u8: Set to 1 (true) if the requested time happened before the earliest error bound of the current system time, otherwise 0 (false).

### Error Response
| 0  1  2  3 |
|:----------:|
|HEADER      |

HEADER: See header definition above. An error response returns the header with T set to Error (0).