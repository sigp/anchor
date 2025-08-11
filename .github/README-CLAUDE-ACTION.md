# Claude Code PR Review Action - Setup Instructions

This document outlines how to set up and configure the Claude Code PR Review GitHub Action for Anchor.

## Prerequisites

1. **AWS IAM Role**: Create an IAM role with the following permissions:
   ```
   {
     "Version": "2012-10-17",
     "Statement": [
       {
         "Effect": "Allow",
         "Action": [
           "bedrock:InvokeModel",
           "bedrock:InvokeModelWithResponseStream",
           "bedrock:ListInferenceProfiles"
         ],
         "Resource": "*"
       }
     ]
   }
   ```

2. **GitHub Repository Settings**:
   - Go to the repository's Settings > Secrets and variables > Actions
   - Add the following secret:
     - `AWS_ROLE_ARN`: The ARN of the IAM role created above

## How to Use

Once configured, Claude will automatically review new Pull Requests and respond to mentions:

1. **Automatic PR Reviews**: Claude will analyze new PRs and provide feedback
2. **Comment-based Interaction**: Mention Claude in PR comments using the trigger phrase `@claude`
   - Example: `@claude Please explain how this code works`
   - Example: `@claude Can you suggest improvements to this implementation?`

## Configuration Details

The workflow is configured with the following settings:

- **Triggers**: 
  - New pull requests
  - Updated pull requests (synchronize)
  - Reopened pull requests
  - Comments containing `@claude`
  
- **AWS Bedrock Settings**:
  - Region: `us-east-1`
  - Model: `us.anthropic.claude-3-7-sonnet-20250219-v1:0`
  - Max output tokens: 4096

- **Timeout**: 30 minutes per interaction

## Troubleshooting

If the action fails:
1. Check the Action logs for error messages
2. Verify the AWS role has appropriate permissions
3. Ensure the AWS role trust policy allows GitHub Actions to assume the role